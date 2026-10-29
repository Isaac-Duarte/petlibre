package io.github.isaacduarte.wellfed

import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.os.Bundle
import io.flutter.embedding.android.FlutterActivity

class MainActivity : FlutterActivity() {
    private val connectivity by lazy { getSystemService(ConnectivityManager::class.java) }

    // Feeders live on the Wi-Fi network, but Android keeps mobile data as
    // the default network while that Wi-Fi has no internet (a feeder's own
    // network, a hotspot), so the device API sessions Rust opens would go
    // out over mobile data and never arrive. The app talks to nothing but
    // feeders, so every socket it opens follows the Wi-Fi network.
    private val wifi =
        object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) {
                // Refused under a VPN that apps cannot bypass: the user
                // must exclude the app from it.
                connectivity.bindProcessToNetwork(network)
            }

            override fun onLost(network: Network) {
                if (connectivity.boundNetworkForProcess == network) {
                    connectivity.bindProcessToNetwork(null)
                }
            }
        }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val request =
            NetworkRequest.Builder()
                .addTransportType(NetworkCapabilities.TRANSPORT_WIFI)
                .removeCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
                .build()
        connectivity.requestNetwork(request, wifi)
    }

    override fun onDestroy() {
        connectivity.unregisterNetworkCallback(wifi)
        connectivity.bindProcessToNetwork(null)
        super.onDestroy()
    }
}
