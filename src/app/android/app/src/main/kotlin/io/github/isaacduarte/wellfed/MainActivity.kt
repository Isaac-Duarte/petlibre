package io.github.isaacduarte.wellfed

import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.net.wifi.WifiManager
import android.net.wifi.WifiNetworkSpecifier
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.PatternMatcher
import android.util.Log
import android.view.WindowManager
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodChannel
import io.reactivex.exceptions.UndeliverableException
import io.reactivex.plugins.RxJavaPlugins
import java.net.Inet4Address

class MainActivity : FlutterActivity() {
    private val connectivity by lazy { getSystemService(ConnectivityManager::class.java) }
    private val main = Handler(Looper.getMainLooper())

    // The owner's Wi-Fi, as Android last gave it.
    private var home: Network? = null

    // The feeder's setup network while this process uses it. A phone with one
    // Wi-Fi radio leaves home for it; a phone that can run two stays on both.
    private var feeder: ConnectivityManager.NetworkCallback? = null

    // Feeders live on the Wi-Fi network, but Android keeps mobile data as
    // the default network while that Wi-Fi has no internet (a feeder's own
    // network, a hotspot), so the device API sessions Rust opens would go
    // out over mobile data and never arrive. The app talks to nothing but
    // feeders, so every socket it opens follows the Wi-Fi network.
    private val wifi =
        object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) {
                // This request also matches the feeder's setup network while
                // this process holds it, and that is not home.
                if (feeder != null) return
                home = network
                // Refused under a VPN that apps cannot bypass: the user
                // must exclude the app from it.
                connectivity.bindProcessToNetwork(network)
            }

            override fun onLost(network: Network) {
                if (home == network) home = null
                // This request matches the feeder's network too: while it is
                // held, an unbound process would send the frame out by default.
                if (feeder == null && connectivity.boundNetworkForProcess == network) {
                    connectivity.bindProcessToNetwork(null)
                }
            }
        }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        current = this
        // The Bluetooth plugin's RxJava throws when a link drops as its stream is
        // cancelled, which the feeder's restart at the end of setup can do, and
        // with no handler that kills the app, and the install with it.
        RxJavaPlugins.setErrorHandler { error ->
            if (error is UndeliverableException) {
                Log.w(TAG, "after its stream ended: ${error.cause}")
            } else {
                Thread.currentThread().let {
                    it.uncaughtExceptionHandler?.uncaughtException(it, error)
                }
            }
        }
        val request =
            NetworkRequest.Builder()
                .addTransportType(NetworkCapabilities.TRANSPORT_WIFI)
                .removeCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
                .build()
        connectivity.requestNetwork(request, wifi)
    }

    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        MethodChannel(flutterEngine.dartExecutor.binaryMessenger, CHANNEL).setMethodCallHandler {
            call,
            result ->
            when (call.method) {
                "phoneAddress" -> result.success(homeAddress())
                "joinFeeder" -> joinFeeder(result)
                "leaveFeeder" -> leaveFeeder(result)
                else -> result.notImplemented()
            }
        }
        MethodChannel(flutterEngine.dartExecutor.binaryMessenger, SCREEN_CHANNEL)
            .setMethodCallHandler { call, result ->
                when (call.method) {
                    "keepOn" -> {
                        keepScreenOn(call.arguments == true)
                        result.success(null)
                    }
                    else -> result.notImplemented()
                }
            }
    }

    // For the length of a run: the flag holds only while this activity is on
    // screen, and needs no permission.
    private fun keepScreenOn(on: Boolean) {
        if (on) {
            window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        } else {
            window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        }
    }

    // The default network may be mobile data, so the address comes from the
    // Wi-Fi network itself.
    private fun homeAddress(): String? {
        val network = home ?: return null
        return connectivity
            .getLinkProperties(network)
            ?.linkAddresses
            ?.map { it.address }
            ?.firstOrNull { it is Inet4Address }
            ?.hostAddress
    }

    // Android shows its own picker for the networks this pattern matches, and
    // answers once the owner chose one. No passphrase: the setup network is open.
    private fun joinFeeder(result: MethodChannel.Result) {
        // A request left from a stopped run answers nothing here, and its network
        // may be gone: only a join this call made says the frame can go.
        feeder?.let { connectivity.unregisterNetworkCallback(it) }
        feeder = null
        home?.let { connectivity.bindProcessToNetwork(it) }
        val wifiManager = getSystemService(WifiManager::class.java)
        Log.i(
            TAG,
            "local-only STA concurrency: ${wifiManager.isStaConcurrencyForLocalOnlyConnectionsSupported}",
        )
        val specifier =
            WifiNetworkSpecifier.Builder()
                .setSsidPattern(PatternMatcher(SETUP_PREFIX, PatternMatcher.PATTERN_PREFIX))
                .build()
        val request =
            NetworkRequest.Builder()
                .addTransportType(NetworkCapabilities.TRANSPORT_WIFI)
                .removeCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
                .setNetworkSpecifier(specifier)
                .build()
        var answered = false
        val callback =
            object : ConnectivityManager.NetworkCallback() {
                override fun onAvailable(network: Network) {
                    connectivity.bindProcessToNetwork(network)
                    if (answered) return
                    answered = true
                    main.post { result.success(null) }
                }

                override fun onUnavailable() {
                    feeder = null
                    if (answered) return
                    answered = true
                    main.post { result.error("unavailable", "no setup network joined", null) }
                }

                // Stays bound to the lost network: a frame sent now fails, where
                // on the home network its password would go to whatever answers.
                // Leaving rebinds.
            }
        feeder = callback
        connectivity.requestNetwork(request, callback, JOIN_TIMEOUT_MS)
    }

    // Answers with this phone's address on its Wi-Fi once it is back there, or
    // null if it does not come back in time. Stock waits about 20 s for its
    // broker, so this does not linger.
    private fun leaveFeeder(result: MethodChannel.Result) {
        feeder?.let { connectivity.unregisterNetworkCallback(it) }
        feeder = null
        home?.let { connectivity.bindProcessToNetwork(it) }

        var left = HOME_WAIT_MS
        val poll =
            object : Runnable {
                override fun run() {
                    val address = homeAddress()
                    if (address != null || left <= 0) {
                        home?.let { connectivity.bindProcessToNetwork(it) }
                        result.success(address)
                        return
                    }
                    left -= HOME_POLL_MS
                    main.postDelayed(this, HOME_POLL_MS)
                }
            }
        poll.run()
    }

    override fun onDestroy() {
        feeder?.let { connectivity.unregisterNetworkCallback(it) }
        feeder = null
        connectivity.unregisterNetworkCallback(wifi)
        // Android can create a new activity before destroying the old one, and
        // the binding belongs to the process: clearing it here would send the
        // new activity's sockets out over mobile data.
        if (current === this) {
            current = null
            connectivity.bindProcessToNetwork(null)
        }
        super.onDestroy()
    }

    private companion object {
        var current: MainActivity? = null

        const val TAG = "wellfed"
        const val CHANNEL = "io.github.isaacduarte.wellfed/wifi"
        const val SCREEN_CHANNEL = "io.github.isaacduarte.wellfed/screen"
        const val SETUP_PREFIX = "PL-AF103-"
        const val JOIN_TIMEOUT_MS = 60_000
        const val HOME_WAIT_MS = 30_000L
        const val HOME_POLL_MS = 250L
    }
}
