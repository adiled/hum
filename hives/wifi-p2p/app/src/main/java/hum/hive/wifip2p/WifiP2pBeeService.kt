package hum.hive.wifip2p

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.IBinder
import android.util.Log
import kotlinx.coroutines.*
import kotlinx.coroutines.sync.Mutex
import java.io.BufferedReader
import java.io.File
import java.io.PrintWriter

/**
 * Android foreground service that runs the WiFi Direct bee.
 *
 * Architecture:
 *   P2P Radio ⇄ P2pManager (TCP on p2p0) ⇄ ThrumClient (to local humd)
 *
 * Inbound P2P tones → translated to chi:"prompt" → sent to humd
 * humd replies (chunk/finish) → collected → sent back over P2P TCP
 */
class WifiP2pBeeService : Service() {
    companion object {
        const val TAG = "HumP2P:Bee"
        const val NOTIFICATION_CHANNEL_ID = "hum-p2p-bee-channel"
        const val NOTIFICATION_ID = 1
        const val REPLY_LIMIT = 4096
        const val DEFAULT_MODEL = "claude-haiku-4.5"
        const val DEFAULT_SYSTEM = "You are a concise assistant for P2P mesh agents."
    }

    private val scope = CoroutineScope(Dispatchers.IO + SupervisorJob())
    private var p2pManager: P2pManager? = null
    private var thrumClient: ThrumClient? = null

    // Per-peer state: we collect chunks per (peerId, sid)
    private val peerReplies = mutableMapOf<String, StringBuilder>()
    private val replyLock = Mutex()

    private val dataDir: String by lazy {
        "${getDataDir().absolutePath}"
    }

    override fun onCreate() {
        super.onCreate()
        Log.i(TAG, "Service created")
        createNotificationChannel()
        startForeground(NOTIFICATION_ID, buildNotification())
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        Log.i(TAG, "Service starting")
        scope.launch {
            startBee()
        }
        return START_STICKY
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onDestroy() {
        Log.i(TAG, "Service stopping")
        p2pManager?.cleanup()
        thrumClient?.disconnect()
        scope.cancel()
        super.onDestroy()
    }

    private fun getDataDir(): File {
        // Use app's internal data directory for identity persistence
        val dir = File(applicationContext.filesDir, "hum")
        dir.mkdirs()
        return dir
    }

    private fun createNotificationChannel() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val channel = NotificationChannel(
                NOTIFICATION_CHANNEL_ID,
                "Hum P2P Bee",
                NotificationManager.IMPORTANCE_LOW,
            ).apply {
                description = "Notification for WiFi Direct bee foreground service"
            }
            val manager = getSystemService(NotificationManager::class.java)
            manager.createNotificationChannel(channel)
        }
    }

    private fun buildNotification(): Notification {
        val channelId = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            NOTIFICATION_CHANNEL_ID
        } else {
            ""
        }
        val builder = Notification.Builder(this, channelId)
            .setContentTitle("Hum P2P Bee")
            .setContentText("WiFi Direct bee is running")
            .setSmallIcon(android.R.drawable.ic_dialog_info)
            .setOngoing(true)
        return builder.build()
    }

    private suspend fun startBee() {
        val sockPath = resolveThrumSock()
        val model = System.getProperty("HUM_P2P_MODEL", DEFAULT_MODEL)
        val systemPrompt = System.getProperty("HUM_P2P_SYSTEM", DEFAULT_SYSTEM)
        val replyLimit = System.getProperty("HUM_P2P_REPLY_LIMIT", "$REPLY_LIMIT").toIntOrNull() ?: REPLY_LIMIT

        // 1. Initialize thrum client
        thrumClient = ThrumClient(sockPath, dataDir, scope)

        // 2. Set up P2P manager
        val p2pConfig = P2pManager.Config(
            port = System.getProperty("HUM_P2P_PORT", "4377").toIntOrNull() ?: 4377,
            serviceType = System.getProperty("HUM_P2P_SERVICE_TYPE", "_hum._tcp"),
            goIntent = System.getProperty("HUM_P2P_GO_INTENT", "8").toIntOrNull() ?: 8,
        )
        val p2p = P2pManager(this, scope, p2pConfig)
        p2pManager = p2p

        // 3. Wire up P2P callbacks
        p2p.onPeerFound = { device ->
            Log.i(TAG, "Peer found: ${device.deviceName} (${device.deviceAddress})")
            // Auto-connect to new peers
            p2p.connectToPeer(device)
        }

        p2p.onGroupFormed = { isGO, ip ->
            Log.i(TAG, "Group formed: GO=$isGO ip=$ip")
        }

        p2p.onPeerConnected = { peerId, reader, writer ->
            Log.i(TAG, "Peer connected: $peerId")
            // Start reading P2P tones and forwarding to humd
            scope.launch {
                handleP2pPeer(peerId, reader, writer, model, systemPrompt, replyLimit)
            }
        }

        p2p.onPeerDisconnected = { peerId ->
            Log.i(TAG, "Peer disconnected: $peerId")
            cleanupPeer(peerId)
        }

        // 4. Forward P2P tones to thrum client
        p2p.onP2pToneReceived = { peerId, line ->
            scope.launch {
                handleInboundP2pTone(peerId, line, model, systemPrompt)
            }
        }

        // 5. Handle thrum replies (chunks for all active sid)
        thrumClient?.onCatchAll { tone ->
            scope.launch {
                handleThrumReply(tone, replyLimit)
            }
        }

        // 6. Start everything
        thrumClient?.connect()
        p2p.init()
        p2p.startDiscovery()

        Log.i(TAG, "Bee started: sock=$sockPath model=$model")
    }

    /**
     * Handle an inbound P2P tone — translate to thrum prompt.
     *
     * P2P wire uses same NDJSON framing as thrum. The tone carries:
     *   {"chi":"prompt","sid":"<peer-hid>","text":"..."}
     *
     * We map this to a thrum chi:"prompt" with a stable sid derived
     * from the peer's hid.
     */
    private suspend fun handleInboundP2pTone(
        peerId: String,
        line: String,
        model: String,
        systemPrompt: String,
    ) {
        try {
            val tone = Tone.fromJson(line)
            when (tone.chi) {
                Chi.PROMPT -> {
                    val text = tone.body["text"] as? String ?: return
                    val peerSid = tone.sid ?: peerId

                    // Create a stable sid for this conversation
                    val convSid = sigil(peerSid, HIVE_NAME)

                    // Send prompt to humd
                    val promptTone = Tone(
                        chi = Chi.PROMPT,
                        rid = rid(),
                        sid = convSid,
                        body = mapOf(
                            "text" to text,
                            "modelId" to model,
                            "systemPrompt" to systemPrompt,
                            "ext" to mapOf(
                                "wifi-p2p" to mapOf(
                                    "peerId" to peerId,
                                )
                            )
                        )
                    )
                    thrumClient?.send(promptTone)
                }
                Chi.CANCEL -> {
                    val sid = tone.sid ?: return
                    val cancelTone = Tone(
                        chi = Chi.CANCEL,
                        rid = rid(),
                        sid = sigil(sid, HIVE_NAME),
                    )
                    thrumClient?.send(cancelTone)
                }
                Chi.CLEANUP -> {
                    val sid = tone.sid ?: return
                    val cleanupTone = Tone(
                        chi = Chi.CLEANUP,
                        rid = rid(),
                        sid = sigil(sid, HIVE_NAME),
                    )
                    thrumClient?.send(cleanupTone)
                }
                Chi.HELLO -> {
                    // P2P hello — acknowledge
                    val peerHid = tone.body["hid"] as? String ?: ""
                    val ack = Tone(
                        chi = Chi.BREATH,
                        rid = rid(),
                        sid = tone.sid,
                        body = mapOf("hid" to peerHid)
                    )
                    p2pManager?.sendToPeer(peerId, ack.toJson())
                }
                Chi.ECHO -> {
                    // Delivery ack — no action needed
                }
                else -> {
                    Log.w(TAG, "Unknown P2P chi: ${tone.chi}")
                }
            }
        } catch (e: Exception) {
            Log.w(TAG, "Failed to parse inbound P2P tone: ${e.message}")
        }
    }

    /**
     * Handle a thrum reply (chunk/finish) and forward back to P2P.
     */
    private suspend fun handleThrumReply(tone: Tone, replyLimit: Int) {
        when (tone.chi) {
            Chi.CHUNK -> {
                val sid = tone.sid ?: return
                val part = tone.body["part"] as? Map<*, *> ?: return
                if (part["type"] == "text") {
                    val text = part["text"] as? String ?: return
                    replyLock.withLock {
                        val buf = peerReplies.getOrPut(sid) { StringBuilder() }
                        buf.append(text)
                    }
                }
            }
            Chi.FINISH -> {
                val sid = tone.sid ?: return
                replyLock.withLock {
                    val buf = peerReplies.remove(sid) ?: return@withLock
                    var reply = buf.toString().trim()
                    if (reply.isEmpty()) reply = "(no reply)"
                    if (reply.length > replyLimit) {
                        reply = reply.take(replyLimit - 3) + "..."
                    }

                    // Send reply back over P2P to all peers for this sid
                    val finishTone = Tone(
                        chi = Chi.FINISH,
                        rid = rid(),
                        sid = sid,
                        body = mapOf("reply" to reply)
                    )
                    p2pManager?.broadcastTone(finishTone.toJson())
                }
            }
            Chi.ERROR -> {
                val sid = tone.sid ?: return
                replyLock.withLock {
                    peerReplies.remove(sid)
                }
                val errTone = Tone(
                    chi = Chi.ERROR,
                    rid = rid(),
                    sid = sid,
                    body = mapOf(
                        "code" to (tone.body["code"] ?: "error"),
                        "message" to (tone.body["message"] ?: "inference failed"),
                    )
                )
                p2pManager?.broadcastTone(errTone.toJson())
            }
        }
    }

    /**
     * Handle a connected P2P peer's TCP stream.
     *
     * Reads NDJSON tones from the TCP connection and routes them
     * through the inbound handler.
     */
    private suspend fun handleP2pPeer(
        peerId: String,
        reader: BufferedReader,
        writer: PrintWriter,
        model: String,
        systemPrompt: String,
        replyLimit: Int,
    ) {
        try {
            var line = reader.readLine()
            while (line != null) {
                if (line.isNotBlank()) {
                    handleInboundP2pTone(peerId, line, model, systemPrompt)
                }
                line = reader.readLine()
            }
        } catch (e: Exception) {
            Log.w(TAG, "P2P peer $peerId disconnected: ${e.message}")
        }
        p2pManager?.onPeerDisconnected?.invoke(peerId)
    }

    private fun cleanupPeer(peerId: String) {
        // Remove any pending replies for this peer
        replyLock.withLock {
            peerReplies.entries.removeIf { (_, _) -> true }
        }
    }

    /**
     * Resolve thrum socket path.
     *
     * Priority:
     *   1. HUM_THRUM_SOCK env (system property on Android)
     *   2. XDG_RUNTIME_DIR/hum/thrum.sock
     *   3. /run/user/<uid>/hum/thrum.sock
     *   4. Default: /data/data/hum.hive.wifip2p/hum/thrum.sock
     */
    private fun resolveThrumSock(): String {
        val explicit = System.getProperty("HUM_THRUM_SOCK")
            ?: System.getProperty("HUM_SOCKET")
        if (explicit != null) return explicit

        val xdgRuntime = System.getProperty("XDG_RUNTIME_DIR")
        if (xdgRuntime != null) {
            return "$xdgRuntime/hum/thrum.sock"
        }

        // Termux default
        val termuxSock = "/data/data/com.termux/files/usr/run/hum/thrum.sock"
        if (File(termuxSock).exists()) return termuxSock

        // Fallback: local bridge TCP
        return "127.0.0.1:14620"
    }
}
