// WiFi Direct: find the phones that run this hive, form a group with one,
// and keep a link to each phone in it. The bee advertises itself as a
// DNS-SD service, _hum._tcp, and asks to connect only to a device that
// advertises it. Once a group forms, the group owner listens on the group
// address and a client dials the owner there; the client dials again
// while the group lasts. A link is one TCP connection carrying the
// stream both ways, known by its <ip>:<port>.

package hum.hive.wifip2p

import android.Manifest
import android.annotation.SuppressLint
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.net.wifi.p2p.WifiP2pConfig
import android.net.wifi.p2p.WifiP2pDevice
import android.net.wifi.p2p.WifiP2pInfo
import android.net.wifi.p2p.WifiP2pManager
import android.net.wifi.p2p.nsd.WifiP2pDnsSdServiceInfo
import android.net.wifi.p2p.nsd.WifiP2pDnsSdServiceRequest
import android.os.Build
import android.os.Looper
import android.util.Log
import androidx.core.content.ContextCompat
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import java.io.Closeable
import java.io.IOException
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.ServerSocket
import java.net.Socket
import java.util.concurrent.ConcurrentHashMap

class P2pManager(
    private val context: Context,
    private val scope: CoroutineScope,
    private val config: Config,
) {
    data class Config(
        val port: Int = 4377,
        val goIntent: Int = 8,
        val serviceType: String = "_hum._tcp",
    )

    var onLine: (peer: String, host: String, line: String) -> Unit = { _, _, _ -> }
    var onGone: (peer: String) -> Unit = {}

    private class Peer(val socket: Socket, val out: Channel<Tone>)

    private val manager: WifiP2pManager? = context.getSystemService(WifiP2pManager::class.java)
    private var channel: WifiP2pManager.Channel? = null
    private val peers = ConcurrentHashMap<String, Peer>()
    private val open = ConcurrentHashMap.newKeySet<Closeable>()
    private var discovery: Job? = null
    private var group: Job? = null
    @Volatile private var formed = false
    private var connecting = false

    fun permitted(): Boolean {
        val needs = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            Manifest.permission.NEARBY_WIFI_DEVICES
        } else {
            Manifest.permission.ACCESS_FINE_LOCATION
        }
        return ContextCompat.checkSelfPermission(context, needs) == PackageManager.PERMISSION_GRANTED
    }

    @SuppressLint("MissingPermission")
    fun start(): Boolean {
        val manager = manager ?: return false
        if (!permitted()) return false
        val channel = manager.initialize(context, Looper.getMainLooper()) {
            Log.w(TAG, "p2p.channel.lost")
        } ?: return false
        this.channel = channel
        val filter = IntentFilter().apply {
            addAction(WifiP2pManager.WIFI_P2P_STATE_CHANGED_ACTION)
            addAction(WifiP2pManager.WIFI_P2P_CONNECTION_CHANGED_ACTION)
        }
        ContextCompat.registerReceiver(context, receiver, filter, ContextCompat.RECEIVER_NOT_EXPORTED)
        manager.setDnsSdResponseListeners(
            channel,
            { _, type, device -> if (type?.startsWith(config.serviceType) == true) found(device) },
            null,
        )
        advertise()
        discovery = scope.launch(Dispatchers.Main) {
            while (isActive) {
                if (!formed) {
                    connecting = false
                    discover()
                }
                delay(REDISCOVER_MS)
            }
        }
        return true
    }

    fun send(peer: String, tone: Tone) {
        val to = peers[peer] ?: return
        if (to.out.trySend(tone).isFailure) runCatching { to.socket.close() }
    }

    @SuppressLint("MissingPermission")
    fun stop() {
        discovery?.cancel()
        disband()
        val manager = manager ?: return
        val channel = channel ?: return
        runCatching { context.unregisterReceiver(receiver) }
        manager.clearServiceRequests(channel, null)
        manager.clearLocalServices(channel, null)
        manager.stopPeerDiscovery(channel, null)
        manager.removeGroup(channel, null)
        channel.close()
    }

    @SuppressLint("MissingPermission")
    private fun advertise() {
        val manager = manager ?: return
        val channel = channel ?: return
        val info = WifiP2pDnsSdServiceInfo.newInstance("hum", config.serviceType, mapOf("port" to "${config.port}"))
        manager.clearLocalServices(channel, done("clearLocalServices") {
            manager.addLocalService(channel, info, done("addLocalService"))
        })
        manager.clearServiceRequests(channel, done("clearServiceRequests") {
            manager.addServiceRequest(channel, WifiP2pDnsSdServiceRequest.newInstance(), done("addServiceRequest"))
        })
    }

    @SuppressLint("MissingPermission")
    private fun discover() {
        val manager = manager ?: return
        val channel = channel ?: return
        manager.discoverServices(channel, done("discoverServices"))
    }

    @SuppressLint("MissingPermission")
    private fun found(device: WifiP2pDevice) {
        val manager = manager ?: return
        val channel = channel ?: return
        if (formed || connecting) return
        connecting = true
        Log.i(TAG, "p2p.found ${device.deviceName} ${device.deviceAddress}")
        val asked = WifiP2pConfig().apply {
            deviceAddress = device.deviceAddress
            groupOwnerIntent = config.goIntent
        }
        manager.connect(channel, asked, object : WifiP2pManager.ActionListener {
            override fun onSuccess() = Unit
            override fun onFailure(reason: Int) {
                connecting = false
                Log.w(TAG, "p2p.connect.failed reason=$reason")
            }
        })
    }

    private fun connected(info: WifiP2pInfo?) {
        val owner = info?.groupOwnerAddress
        if (info == null || !info.groupFormed || owner == null) {
            if (formed) Log.i(TAG, "p2p.group.gone")
            formed = false
            disband()
            return
        }
        if (formed) return
        formed = true
        connecting = false
        Log.i(TAG, "p2p.group.formed owner=${info.isGroupOwner} at=${owner.hostAddress}")
        group = scope.launch { if (info.isGroupOwner) serve(owner) else reach(owner) }
    }

    private fun disband() {
        group?.cancel()
        group = null
        for (closing in open) runCatching { closing.close() }
        open.clear()
        for (peer in peers.values) runCatching { peer.socket.close() }
    }

    private suspend fun serve(at: InetAddress) {
        try {
            val server = ServerSocket().also { open += it }
            server.reuseAddress = true
            server.bind(InetSocketAddress(at, config.port), BACKLOG)
            Log.i(TAG, "p2p.listening ${at.hostAddress}:${config.port}")
            while (currentCoroutineContext().isActive) link(server.accept())
        } catch (e: IOException) {
            Log.i(TAG, "p2p.listen.stopped ${e.message}")
        }
    }

    private suspend fun reach(owner: InetAddress) {
        var tries = 0
        while (formed && currentCoroutineContext().isActive) {
            val socket = Socket().also { open += it }
            try {
                socket.connect(InetSocketAddress(owner, config.port), DIAL_MS)
                tries = 0
                link(socket).join()
            } catch (e: IOException) {
                Log.d(TAG, "p2p.dial.failed ${e.message}")
            } finally {
                open -= socket
                runCatching { socket.close() }
            }
            delay(ThrumClient.backoff(tries++))
        }
    }

    private fun link(socket: Socket): Job {
        socket.keepAlive = true
        socket.tcpNoDelay = true
        val host = socket.inetAddress.hostAddress ?: "?"
        val id = "$host:${socket.port}"
        val out = Channel<Tone>(OUTBOX)
        peers[id] = Peer(socket, out)
        Log.i(TAG, "p2p.link $id")
        scope.launch {
            try {
                val to = socket.getOutputStream()
                for (tone in out) {
                    to.write(line(tone))
                    to.flush()
                }
            } catch (_: IOException) {
            } finally {
                runCatching { socket.close() }
            }
        }
        return scope.launch {
            try {
                val lines = Lines(socket.getInputStream())
                while (true) onLine(id, host, lines.next() ?: break)
            } catch (_: IOException) {
            } finally {
                peers.remove(id)
                out.close()
                runCatching { socket.close() }
                Log.i(TAG, "p2p.unlink $id")
                onGone(id)
            }
        }
    }

    private fun done(what: String, then: () -> Unit = {}) = object : WifiP2pManager.ActionListener {
        override fun onSuccess() = then()
        override fun onFailure(reason: Int) {
            Log.w(TAG, "p2p.$what.failed reason=$reason")
        }
    }

    private val receiver = object : BroadcastReceiver() {
        @SuppressLint("MissingPermission")
        override fun onReceive(ctx: Context, intent: Intent) {
            when (intent.action) {
                WifiP2pManager.WIFI_P2P_STATE_CHANGED_ACTION -> {
                    val state = intent.getIntExtra(WifiP2pManager.EXTRA_WIFI_STATE, -1)
                    Log.i(TAG, "p2p.enabled=${state == WifiP2pManager.WIFI_P2P_STATE_ENABLED}")
                    if (state == WifiP2pManager.WIFI_P2P_STATE_ENABLED) advertise()
                }
                WifiP2pManager.WIFI_P2P_CONNECTION_CHANGED_ACTION -> {
                    val manager = manager ?: return
                    val channel = channel ?: return
                    manager.requestConnectionInfo(channel) { connected(it) }
                }
            }
        }
    }

    companion object {
        const val TAG = "HumP2P"
        private const val REDISCOVER_MS = 60_000L
        private const val DIAL_MS = 5_000
        private const val BACKLOG = 16
        private const val OUTBOX = 256
    }
}
