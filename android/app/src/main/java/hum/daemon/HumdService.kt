package hum.daemon

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Environment
import android.os.IBinder
import android.util.Log
import androidx.core.app.NotificationCompat
import java.io.File
import java.io.FileOutputStream
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Hosts the native hum daemon for Android.
 *
 * "humd for Android" is the actual Rust `humd` from the workspace,
 * cross-compiled to a PIE ELF (see android/scripts/build-humd.sh) and
 * bundled as an asset. This service is the host: it extracts the binary,
 * points every XDG path at app-private storage, writes a minimal
 * hum.json (+ optional peers.json with the companion machine's iroh
 * hint), and spawns the daemon as a supervised child process. No
 * Termux, no root, no JNI — the daemon is a plain exec.
 *
 * humd's boot is entirely XDG-driven (hum-paths), so the Android port
 * needs zero Rust changes. The ensemble mesh (iroh QUIC + relay) is
 * the overlay that reaches the machine's humd; WifiDirectManager is
 * the radio underlay for ad-hoc peer discovery.
 */
class HumdService : Service() {
    companion object {
        const val TAG = "humd.android"
        const val CHANNEL = "humd"
        private const val BIN_ASSET = "humd"
    }

    private var proc: Process? = null
    private val alive = AtomicBoolean(true)
    private var wifi: WifiDirectManager? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        val nm = getSystemService(NOTIFICATION_SERVICE) as NotificationManager
        nm.createNotificationChannel(
            NotificationChannel(CHANNEL, "humd", NotificationManager.IMPORTANCE_LOW)
        )
        startForegroundCompat()
        ensureBinary()
        writeConfig()
        startHumd()
        // Radio underlay for ad-hoc phone↔phone discovery; the ensemble
        // overlay (iroh relay) needs no WiFi Direct to reach the machine.
        wifi = WifiDirectManager(this).also {
            it.onGroupFormed = { isGO, ip ->
                Log.i(TAG, "p2p link up: go=$isGO ip=$ip")
            }
            if (it.start()) Log.i(TAG, "wifi-direct underlay started")
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        // Restart if the OS kills us; the daemon process is supervised by
        // the service and does not die with it.
        startForegroundCompat()
        if (proc == null || !proc!!.isAlive) startHumd()
        return START_STICKY
    }

    override fun onDestroy() {
        alive.set(false)
        wifi?.stop()
        proc?.destroy()
        proc?.waitFor(500, java.util.concurrent.TimeUnit.MILLISECONDS)
        super.onDestroy()
    }

    // ── foreground surface ─────────────────────────────────────────────────
    private fun startForegroundCompat() {
        val intent = Intent(this, HumdService::class.java)
        val pi = PendingIntent.getActivity(this, 0, intent, PendingIntent.FLAG_IMMUTABLE)
        val n = NotificationCompat.Builder(this, CHANNEL)
            .setContentTitle(getString(R.string.notif_title))
            .setContentText(getString(R.string.notif_text))
            .setSmallIcon(android.R.drawable.stat_sys_data_paused)
            .setContentIntent(pi)
            .setOngoing(true)
            .build()
        if (android.os.Build.VERSION.SDK_INT >= 34) {
            startForeground(1, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        } else {
            startForeground(1, n)
        }
    }

    // ── binary ─────────────────────────────────────────────────────────────
    private fun ensureBinary() {
        val files = filesDir
        val dest = File(files, BIN_ASSET)
        // Idempotent: re-extract only when the bundled asset is bigger
        // than what we already placed (i.e. a fresh build shipped a new
        // binary). Otherwise reuse the installed copy.
        val bundled = assets.open(BIN_ASSET).use { it.available() }
        if (dest.exists() && dest.length() >= bundled && dest.length() > 0) {
            dest.setExecutable(true, true)
            Log.i(TAG, "humd binary already bundled (${dest.length()} bytes)")
            return
        }
        assets.open(BIN_ASSET).use { input ->
            FileOutputStream(dest).use { output -> input.copyTo(output) }
        }
        dest.setExecutable(true, true)
        Log.i(TAG, "extracted humd binary -> ${dest.absolutePath} (${dest.length()} bytes)")
    }

    // ── config ─────────────────────────────────────────────────────────────
    /** Write a minimal valid hum.json (all sections default) plus, if a
     *  companion machine's iroh hint is provided, peers.json so the phone
     *  daemon dials it on boot. */
    private fun writeConfig() {
        val cfg = File(configDir(), "hum").apply { mkdirs() }
        val humJson = File(cfg, "hum.json")
        if (!humJson.exists()) {
            humJson.writeText(
                """
                {
                  "humd": { "permissionDuskMs": 60000, "driftRetentionDays": 7 },
                  "fs": { "roots": [ { "path": "~/code", "mode": "rw" } ], "denied": [] },
                  "nest": { "maxActiveCells": 1, "cellIdlePruneThresholdMs": 300000, "default": "" }
                }
                """.trimIndent()
            )
        }

        // Optional bootstrap: the companion machine's humd id + iroh hint.
        val peerHint = System.getProperty("hum.peerHint") // "humd_id,iroh:nodeid"
        if (peerHint != null) {
            val (id, hint) = peerHint.split(",", limit = 2)
            val peers = File(cfg, "peers.json")
            if (!peers.exists()) {
                peers.writeText(
                    """{"peers":[{"humd_id":"$id","hints":["$hint"]}]}"""
                )
            }
        }
    }

    // ── daemon ─────────────────────────────────────────────────────────────
    private fun startHumd() {
        val files = filesDir
        val bin = File(files, BIN_ASSET)
        if (!bin.exists()) {
            Log.e(TAG, "humd binary missing — run android/scripts/build-humd.sh")
            return
        }

        val pb = ProcessBuilder(bin.absolutePath)
        pb.directory(files)
        pb.redirectErrorStream(true)
        pb.redirectOutput(ProcessBuilder.Redirect.appendTo(File(files, "humd.log")))

        // hum-paths resolves everything from XDG env vars; pin them to
        // app-private storage so the daemon's key/socket/config stay inside.
        val env = pb.environment()
        env["HOME"] = files.absolutePath
        env["XDG_STATE_HOME"] = File(files, "state").absolutePath
        env["XDG_CONFIG_HOME"] = File(files, "config").absolutePath
        env["XDG_DATA_HOME"] = File(files, "data").absolutePath
        env["XDG_CACHE_HOME"] = File(files, "cache").absolutePath
        env["XDG_RUNTIME_DIR"] = File(files, "runtime").absolutePath
        env["HUM_LOG_LEVEL"] = "info"

        try {
            proc = pb.start()
            Log.i(TAG, "humd spawned pid=${proc!!.pid()}")
        } catch (e: Exception) {
            Log.e(TAG, "spawn failed: ${e.message}")
            return
        }

        // Supervise: if the daemon dies unexpectedly, log and respawn
        // (bounded) unless the service itself is tearing down.
        Thread {
            while (alive.get()) {
                try {
                    proc!!.waitFor()
                    if (alive.get()) {
                        Log.w(TAG, "humd exited rc=${proc!!.exitValue()}; respawning")
                        Thread.sleep(2000)
                        startHumd()
                    }
                    break
                } catch (e: InterruptedException) {
                    break
                }
            }
        }.apply { isDaemon = true; start() }
    }

    // ── path helpers ───────────────────────────────────────────────────────
    private fun configDir() = File(filesDir, "config")
}
