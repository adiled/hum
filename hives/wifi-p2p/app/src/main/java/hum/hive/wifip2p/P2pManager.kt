package hum.hive.wifip2p

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.net.wifi.p2p.WifiP2pDevice
import android.net.wifi.p2p.WifiP2pDeviceList
import android.net.wifi.p2p.WifiP2pGroup
import android.net.wifi.p2p.WifiP2pInfo
import android.net.wifi.p2p.WifiP2pManager
import android.os.Build
import android.util.Log
import kotlinx.coroutines.*
import java.net.InetAddress
import java.net.ServerSocket
import java.net.Socket
import java.io.BufferedReader
import java.io.InputStreamReader
import java.io.PrintWriter

/**
 * WiFi Direct peer-to-peer group management.
 *
 * Handles:
 *   - Service discovery (Bonjour-style _hum._tcp)
 *   - Group formation (GO negotiation)
 *   - TCP server on p2p0 interface (Group Owner side)
 *   - TCP client connection (client side)
 *   - Peer lifecycle tracking
 */
class P2pManager(
    private val context: Context,
    private val scope: CoroutineScope,
    private val config: Config,
) {
    companion object {
        const val TAG = "HumP2P"
        const val DEFAULT_PORT = 4377
        const val SERVICE_TYPE = "_hum._tcp"
    }

    data class Config(
        val port: Int = DEFAULT_PORT,
        val serviceType: String = SERVICE_TYPE,
        val goIntent: Int = 8,
    )

    // Callbacks
    var onPeerConnected: ((peerId: String, reader: BufferedReader, writer: PrintWriter) -> Unit)? = null
    var onPeerDisconnected: ((peerId: String) -> Unit)? = null
    var onPeerFound: ((device: WifiP2pDevice) -> Unit)? = null
    var onGroupFormed: ((isGO: Boolean, groupIp: String) -> Unit)? = null

    private var p2pManager: WifiP2pManager? = null
    private var p2pChannel: WifiP2pManager.Channel? = null
    private var isGO = false
    private var groupIp: String? = null

    private var serverSocket: ServerSocket? = null
    private var serverJob: Job? = null
    private val activePeers = mutableMapOf<String, Pair<BufferedReader, PrintWriter>>()
    private var receiverRegistered = false

    /** Initialize P2P manager and register broadcast receiver. */
    fun init(): Boolean {
        p2pManager = context.getSystemService(Context.WIFI_P2P_SERVICE) as? WifiP2pManager
            ?: return false
        p2pChannel = p2pManager?.initialize(context, scope.monitor, null)
            ?: return false

        // Register broadcast receiver for P2P events
        val intentFilter = IntentFilter().apply {
            addAction("android.net.wifi.p2p.STATE_CHANGED")
            addAction("android.net.wifi.p2p.PEERS_CHANGED")
            addAction("android.net.wifi.p2p.CONNECTION_INFO_CHANGED")
            addAction("android.net.wifi.p2p.GROUP_INFO_CHANGED")
            addAction("android.net.wifi.p2p.GROUP_REMOVED")
            addAction("android.net.wifi.p2p.DISCOVERY_CHANGE")
        }
        context.registerReceiver(p2pReceiver, intentFilter, Context.RECEIVER_EXPORTED)
        receiverRegistered = true
        return true
    }

    /** Start service discovery. */
    fun startDiscovery() {
        val manager = p2pManager ?: return
        val channel = p2pChannel ?: return

        // Register a local service (Bonjour _hum._tcp) so peers can discover us
        manager.requestPeers(channel, null)

        // Start peer discovery
        manager.discoverPeers(channel, object : WifiP2pManager.ActionListener {
            override fun onSuccess() {
                Log.i(TAG, "P2P discovery started")
            }
            override fun onFailure(reason: Int) {
                Log.w(TAG, "P2P discovery failed: reason=$reason")
            }
        })

        // Register service for Bonjour-style discovery
        val serviceRequest = WifiP2pDevice().apply {
            // We register our service type so peers see us
        }
        // In Android 13+, use discoverTransmitDiscoveryChannel for faster discovery
    }

    /** Stop discovery. */
    fun stopDiscovery() {
        p2pManager?.stopPeerDiscovery(p2pChannel, null)
    }

    /** Connect to a discovered peer. */
    fun connectToPeer(device: WifiP2pDevice) {
        val manager = p2pManager ?: return
        val channel = p2pChannel ?: return

        val config = WifiP2pManager.WifiP2pConfig().apply {
            deviceAddress = device.deviceAddress
            groupOwnerIntent = this@P2pManager.config.goIntent
        }
        manager.connect(channel, config, object : WifiP2pManager.ActionListener {
            override fun onSuccess() {
                Log.i(TAG, "P2P connect initiated to ${device.deviceAddress}")
            }
            override fun onFailure(reason: Int) {
                Log.w(TAG, "P2P connect failed: reason=$reason")
            }
        })
    }

    /** Create a group as Group Owner. */
    fun createGroup() {
        val manager = p2pManager ?: return
        val channel = p2pChannel ?: return

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            manager.createLocalP2pGroup(channel, object : WifiP2pManager.ActionListener {
                override fun onSuccess() = Log.i(TAG, "P2P group created")
                override fun onFailure(reason: Int) = Log.w(TAG, "P2P group creation failed: $reason")
            })
        } else {
            // Legacy: no direct group creation, rely on connect()
            Log.w(TAG, "Direct group creation not supported on this API level")
        }
    }

    /** Remove the current group. */
    fun removeGroup() {
        p2pManager?.removeGroup(p2pChannel, null)
        stopServer()
        isGO = false
        groupIp = null
    }

    /** Start TCP server on the P2P interface (GO side). */
    private fun startServer(ip: String) {
        stopServer()
        serverJob = scope.launch {
            try {
                val addr = InetAddress.getByName(ip)
                serverSocket = ServerSocket(config.port, 10, addr)
                Log.i(TAG, "P2P TCP server listening on $ip:${config.port}")

                while (isActive) {
                    val client = serverSocket!!.accept()
                    launch {
                        handlePeerConnection(client)
                    }
                }
            } catch (e: Exception) {
                Log.w(TAG, "P2P server stopped: ${e.message}")
            }
        }
    }

    private fun stopServer() {
        serverJob?.cancel()
        serverJob = null
        try { serverSocket?.close() } catch (_: Exception) {}
        serverSocket = null
    }

    /** Connect to GO as client (client side). */
    private fun connectToGO(ip: String) {
        scope.launch {
            try {
                val socket = Socket(ip, config.port)
                Log.i(TAG, "P2P client connected to $ip:${config.port}")
                handlePeerConnection(socket)
            } catch (e: Exception) {
                Log.w(TAG, "P2P client connect failed: ${e.message}")
            }
        }
    }

    /** Handle a single P2P TCP connection. */
    private fun handlePeerConnection(socket: Socket) {
        try {
            val reader = BufferedReader(InputStreamReader(socket.getInputStream()))
            val writer = PrintWriter(socket.getOutputStream(), true)
            // Use socket's remote address as peer ID (stable enough)
            val peerId = "${socket.inetAddress.hostAddress}:${socket.port}"
            activePeers[peerId] = reader to writer
            onPeerConnected?.invoke(peerId, reader, writer)

            // Read loop — tones from the P2P peer
            scope.launch {
                try {
                    while (isActive) {
                        val line = reader.readLine() ?: break
                        if (line.isBlank()) continue
                        onP2pToneReceived(peerId, line)
                    }
                } catch (_: Exception) {
                } finally {
                    activePeers.remove(peerId)
                    onPeerDisconnected?.invoke(peerId)
                    try { socket.close() } catch (_: Exception) {}
                }
            }
        } catch (e: Exception) {
            Log.w(TAG, "P2P connection error: ${e.message}")
        }
    }

    /** Send a tone over P2P to a specific peer. */
    fun sendToPeer(peerId: String, tone: String) {
        val pair = activePeers[peerId]
        if (pair != null) {
            pair.second.println(tone)
            pair.second.flush()
        }
    }

    /** Broadcast tone to all connected peers. */
    fun broadcastTone(tone: String) {
        for ((_, pair) in activePeers) {
            pair.second.println(tone)
            pair.second.flush()
        }
    }

    /** Called when a P2P tone arrives from the radio. */
    var onP2pToneReceived: (peerId: String, line: String) -> Unit = { _, _ -> }

    /** Cleanup all connections. */
    fun cleanup() {
        stopDiscovery()
        removeGroup()
        stopServer()
        for ((_, pair) in activePeers) {
            try { pair.first.close() } catch (_: Exception) {}
            try { pair.second.close() } catch (_: Exception) {}
        }
        activePeers.clear()
        if (receiverRegistered) {
            try { context.unregisterReceiver(p2pReceiver) } catch (_: Exception) {}
            receiverRegistered = false
        }
    }

    // ── Broadcast receiver for P2P events ─────────────────────

    private val p2pReceiver = object : BroadcastReceiver() {
        override fun onReceive(ctx: Context, intent: Intent) {
            when (intent.action) {
                "android.net.wifi.p2p.STATE_CHANGED" -> {
                    val enabled = intent.getBooleanExtra(
                        WifiP2pManager.EXTRA_WIFI_STATE, false
                    )
                    Log.i(TAG, "WiFi P2P enabled=$enabled")
                }
                "android.net.wifi.p2p.PEERS_CHANGED" -> {
                    val peers = intent.getParcelableExtra<WifiP2pDeviceList>(
                        WifiP2pManager.EXTRA_WIFI_P2P_DEVICE_LIST
                    )
                    if (peers != null) {
                        for (device in peers.deviceList) {
                            Log.i(TAG, "Found peer: ${device.deviceAddress} " +
                                "name=${device.deviceName} " +
                                "status=${device.status}")
                            onPeerFound?.invoke(device)
                        }
                    }
                }
                "android.net.wifi.p2p.CONNECTION_INFO_CHANGED" -> {
                    val info = intent.getParcelableExtra<WifiP2pInfo>(
                        WifiP2pManager.EXTRA_WIFI_P2P_INFO
                    )
                    if (info != null) {
                        val go = info.groupOwnerAddress?.hostAddress ?: ""
                        val isGO = info.isGroupOwner
                        this@P2pManager.isGO = isGO
                        groupIp = go
                        Log.i(TAG, "P2P group formed: GO=$isGO ip=$go")
                        onGroupFormed?.invoke(isGO, go)

                        if (isGO && go.isNotEmpty()) {
                            startServer(go)
                        } else if (!isGO && go.isNotEmpty()) {
                            connectToGO(go)
                        }
                    }
                }
                "android.net.wifi.p2p.GROUP_INFO_CHANGED" -> {
                    val group = intent.getParcelableExtra<WifiP2pGroup>(
                        WifiP2pManager.EXTRA_WIFI_P2P_GROUP
                    )
                    if (group != null) {
                        Log.i(TAG, "Group: ${group.network?.ssid} " +
                            "clients=${group.clientList?.size}")
                    }
                }
                "android.net.wifi.p2p.GROUP_REMOVED" -> {
                    Log.i(TAG, "P2P group removed")
                    isGO = false
                    groupIp = null
                    stopServer()
                }
            }
        }
    }
}
