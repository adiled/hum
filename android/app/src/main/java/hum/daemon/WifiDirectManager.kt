package hum.daemon

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

/**
 * WiFi Direct radio underlay for the native humd.
 *
 * The ensemble mesh (iroh QUIC + relay) is the *overlay* that reaches
 * other humds — it works over any interface, including p2p0. WiFi
 * Direct is the *underlay* that gives phones an ad-hoc L2 link without
 * a router or internet: discovery, group formation, and the p2p0 IP.
 *
 * When a group forms, [onGroupFormed] hands the p2p0 address up to the
 * host so the daemon can reach phone-to-phone peers directly (the
 * meetup case). For phone-to-machine, iroh's relay + hole-punching
 * needs no WiFi Direct at all.
 */
class WifiDirectManager(private val context: Context) {
    companion object {
        const val TAG = "humd.wifidirect"
    }

    var onPeerFound: (WifiP2pDevice) -> Unit = { }
    var onGroupFormed: (isGroupOwner: Boolean, p2pIp: String?) -> Unit = { _, _ -> }

    private var manager: WifiP2pManager? = null
    private var channel: WifiP2pManager.Channel? = null
    private var receiverRegistered = false

    fun start(): Boolean {
        manager = context.getSystemService(Context.WIFI_P2P_SERVICE) as? WifiP2pManager
            ?: run { Log.w(TAG, "no WifiP2p service on this device"); return false }
        channel = manager?.initialize(context, context.mainLooper, null)
            ?: return false

        context.registerReceiver(
            receiver,
            IntentFilter().apply {
                addAction(WifiP2pManager.WIFI_P2P_STATE_CHANGED_ACTION)
                addAction(WifiP2pManager.WIFI_P2P_PEERS_CHANGED_ACTION)
                addAction(WifiP2pManager.WIFI_P2P_CONNECTION_CHANGED_ACTION)
            },
            Context.RECEIVER_EXPORTED
        )
        receiverRegistered = true
        Log.i(TAG, "wifi-direct underlay up")
        discover()
        return true
    }

    fun stop() {
        manager?.stopPeerDiscovery(channel, null)
        if (receiverRegistered) {
            try { context.unregisterReceiver(receiver) } catch (_: Exception) {}
            receiverRegistered = false
        }
    }

    private fun discover() {
        val m = manager ?: return
        val c = channel ?: return
        m.discoverPeers(c, object : WifiP2pManager.ActionListener {
            override fun onSuccess() = Log.i(TAG, "p2p discovery started")
            override fun onFailure(reason: Int) = Log.w(TAG, "p2p discovery failed reason=$reason")
        })
    }

    fun connectTo(device: WifiP2pDevice) {
        val m = manager ?: return
        val c = channel ?: return
        val cfg = WifiP2pManager.WifiP2pConfig().apply {
            deviceAddress = device.deviceAddress
        }
        m.connect(c, cfg, object : WifiP2pManager.ActionListener {
            override fun onSuccess() = Log.i(TAG, "p2p connect initiated -> ${device.deviceName}")
            override fun onFailure(reason: Int) = Log.w(TAG, "p2p connect failed reason=$reason")
        })
    }

    private val receiver = object : BroadcastReceiver() {
        override fun onReceive(ctx: Context, intent: Intent) {
            when (intent.action) {
                WifiP2pManager.WIFI_P2P_PEERS_CHANGED_ACTION -> {
                    val peers = intent.getParcelableExtra<WifiP2pDeviceList>(
                        WifiP2pManager.EXTRA_WIFI_P2P_DEVICE_LIST
                    ) ?: return
                    for (d in peers.deviceList) {
                        Log.i(TAG, "found peer ${d.deviceName} @ ${d.deviceAddress}")
                        onPeerFound(d)
                    }
                }
                WifiP2pManager.WIFI_P2P_CONNECTION_CHANGED_ACTION -> {
                    val info = intent.getParcelableExtra<WifiP2pInfo>(
                        WifiP2pManager.EXTRA_WIFI_P2P_INFO
                    ) ?: return
                    val ip = info.groupOwnerAddress?.hostAddress
                    Log.i(TAG, "group formed: go=${info.isGroupOwner} ip=$ip")
                    onGroupFormed(info.isGroupOwner, ip)
                }
                WifiP2pManager.WIFI_P2P_STATE_CHANGED_ACTION -> {
                    val on = intent.getIntExtra(WifiP2pManager.EXTRA_WIFI_STATE, -1) ==
                        WifiP2pManager.WIFI_P2P_STATE_ENABLED
                    Log.i(TAG, "wifi-p2p radio enabled=$on")
                    if (on) discover()
                }
            }
        }
    }
}
