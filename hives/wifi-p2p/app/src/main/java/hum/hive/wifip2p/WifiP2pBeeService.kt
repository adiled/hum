// The bee: a foreground service that keeps one forager nestled on humd and
// the WiFi Direct group's links open, and hands what crosses between them
// to the forage. Its id, its key and its settings are in the app's files
// directory, under hum/.

package hum.hive.wifip2p

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.IBinder
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import java.io.File

const val HIVE = "wifi-p2p"

val HELLO: List<Pair<String, Any?>> = listOf(
    "version" to BuildConfig.VERSION_NAME,
    "propensity" to mapOf(
        "statefulness" to "convention-stateful",
        "richness" to "lean",
        "wire" to "wifi-p2p/tcp",
    ),
    "chis" to listOf(Chi.HELLO, Chi.PROMPT, Chi.CANCEL, Chi.CLEANUP, Chi.UNFURL, Chi.WILT, Chi.ERROR),
    "source" to "https://github.com/adiled/hum/tree/main/hives/wifi-p2p",
)

class WifiP2pBeeService : Service() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private var client: ThrumClient? = null
    private var p2p: P2pManager? = null

    override fun onCreate() {
        super.onCreate()
        val channel = NotificationChannel(CHANNEL, getString(R.string.service_notification_channel), NotificationManager.IMPORTANCE_LOW)
        getSystemService(NotificationManager::class.java).createNotificationChannel(channel)
        ServiceCompat.startForeground(
            this,
            NOTIFICATION,
            notification(getString(R.string.service_notification_text)),
            ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE,
        )
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (client == null) start()
        return START_STICKY
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onDestroy() {
        p2p?.stop()
        client?.stop()
        scope.cancel()
        super.onDestroy()
    }

    private fun start() {
        val dir = File(filesDir, "hum")
        val key = try {
            IdentityStore.load(dir)
        } catch (e: Exception) {
            return quit("no bee in $dir: ${e.message}")
        }
        val settings = Settings.load(File(dir, "$HIVE.json"))
        val humd = Humd.parse(settings.humd) ?: return quit("humd ${settings.humd} is neither /<socket> nor <host>:<port>")
        val p2p = P2pManager(this, scope, settings.p2p)
        lateinit var client: ThrumClient
        val forage = Forage(settings.forage, humd = { client.send(it) }, peer = p2p::send)
        client = ThrumClient(dial(humd), HIVE, Role.FORAGER, key, HELLO) { event ->
            when (event) {
                ThrumClient.Event.Nestled -> say("nestled on humd at ${settings.humd}")
                ThrumClient.Event.Astray -> {
                    forage.astray()
                    say("humd went away; dialing ${settings.humd} again")
                }
                is ThrumClient.Event.Heard -> {
                    advisory(event.tone)
                    forage.hum(event.tone)
                }
                is ThrumClient.Event.Refused -> quit("humd refused ${key.hid}: ${event.why}")
                is ThrumClient.Event.Failing -> say("humd is not answering at ${settings.humd} (${event.tries} tries): ${event.why}")
            }
        }
        p2p.onLine = forage::heard
        p2p.onGone = forage::gone
        if (!p2p.start()) return quit("WiFi Direct is unavailable, or nearby-devices permission is not granted")
        this.client = client
        this.p2p = p2p
        client.start(scope)
        Log.i(TAG, "bee ${key.hid} dialing humd at ${settings.humd}")
    }

    private fun advisory(tone: Tone) {
        if (tone.str("chi") != Chi.LOG) return
        val fields = tone["fields"] as? Tone ?: return
        Log.w(TAG, "advisory ${fields.str("advisory")}: ${tone.str("message")}")
    }

    private fun say(what: String) {
        Log.i(TAG, what)
        getSystemService(NotificationManager::class.java).notify(NOTIFICATION, notification(what))
    }

    private fun quit(why: String) {
        Log.e(TAG, why)
        stopSelf()
    }

    private fun notification(text: String) = NotificationCompat.Builder(this, CHANNEL)
        .setContentTitle(getString(R.string.service_notification_title))
        .setContentText(text)
        .setSmallIcon(android.R.drawable.stat_notify_sync)
        .setOngoing(true)
        .build()

    companion object {
        const val TAG = "HumP2P"
        private const val CHANNEL = "hum-p2p-bee"
        private const val NOTIFICATION = 1
    }
}
