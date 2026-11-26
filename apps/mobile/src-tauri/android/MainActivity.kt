// Keeps the page clear of the phone's bars and keyboard, the bars in its color, its touches felt, and its alerts told.
package dev.cursorbyok.mobile

import android.Manifest
import android.app.NotificationManager
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Color
import android.os.Build
import android.os.Bundle
import android.os.VibrationEffect
import android.os.Vibrator
import android.view.View
import android.webkit.JavascriptInterface
import android.webkit.WebView
import androidx.activity.enableEdgeToEdge
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat

class MainActivity : TauriActivity() {
  @Volatile private var seen = false
  @Volatile private var touched: String? = null
  private var asked = false
  /** What the page last asked to show while it works; kept until the app is in sight to start it. */
  private var working = ""

  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    Work.channels(this)
    touched = intent.getStringExtra(Work.SESSION)
    val content = findViewById<View>(android.R.id.content)
    ViewCompat.setOnApplyWindowInsetsListener(content) { view, insets ->
      val taken = insets.getInsets(
        WindowInsetsCompat.Type.systemBars()
          or WindowInsetsCompat.Type.displayCutout()
          or WindowInsetsCompat.Type.ime()
      )
      view.setPadding(taken.left, taken.top, taken.right, taken.bottom)
      WindowInsetsCompat.CONSUMED
    }
  }

  override fun onNewIntent(intent: Intent) {
    super.onNewIntent(intent)
    intent.getStringExtra(Work.SESSION)?.let { touched = it }
  }

  override fun onStart() {
    super.onStart()
    seen = true
    if (working.isNotEmpty() && !Work.running) keepAwake()
  }

  override fun onDestroy() {
    // Without its page nothing would end the service.
    if (isFinishing) stopService(Intent(this, Work::class.java))
    super.onDestroy()
  }

  private fun keepAwake() {
    mayTell()
    startForegroundService(Intent(this, Work::class.java).putExtra(Work.TEXT, working))
  }

  override fun onStop() {
    seen = false
    super.onStop()
  }

  override fun onWebViewCreate(webView: WebView) {
    webView.addJavascriptInterface(Bars(), "Bars")
    webView.addJavascriptInterface(Feel(), "Feel")
    webView.addJavascriptInterface(Alerts(), "Alerts")
  }

  /** Asked once a run, when there is first something to tell; Android 13 shows nothing without it. */
  private fun mayTell() {
    if (asked || Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) return
    asked = true
    if (checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) {
      requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), 1)
    }
  }

  inner class Alerts {
    @JavascriptInterface
    fun tell(session: String, title: String, text: String) = Work.tell(this@MainActivity, session, title, text)

    /** Empty text stops keeping the app awake. */
    @JavascriptInterface
    fun working(text: String) = runOnUiThread {
      working = text
      when {
        text.isEmpty() -> stopService(Intent(this@MainActivity, Work::class.java))
        Work.running -> Work.update(this@MainActivity, text)
        // Android lets a foreground service start only while the app is in sight.
        seen -> keepAwake()
      }
    }

    @JavascriptInterface
    fun clear() {
      val alerts = getSystemService(NotificationManager::class.java)
      for (alert in alerts.activeNotifications) if (alert.tag == Work.CHAT_TAG) alerts.cancel(alert.tag, alert.id)
    }

    /** The conversation of the alert last touched, once. */
    @JavascriptInterface
    fun opened(): String = touched.orEmpty().also { touched = null }
  }

  inner class Bars {
    @JavascriptInterface
    fun paint(color: String, light: Boolean) = runOnUiThread {
      val content = findViewById<View>(android.R.id.content)
      content.setBackgroundColor(Color.parseColor(color))
      val bars = WindowCompat.getInsetsController(window, content)
      bars.isAppearanceLightStatusBars = light
      bars.isAppearanceLightNavigationBars = light
    }
  }

  inner class Feel {
    private val vibrator = getSystemService(Vibrator::class.java)

    @JavascriptInterface
    fun play(kind: String) {
      if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
        val effect = when (kind) {
          "send", "confirm" -> VibrationEffect.EFFECT_CLICK
          "hold", "done" -> VibrationEffect.EFFECT_HEAVY_CLICK
          "warn" -> VibrationEffect.EFFECT_DOUBLE_CLICK
          else -> VibrationEffect.EFFECT_TICK
        }
        vibrator.vibrate(VibrationEffect.createPredefined(effect))
      } else {
        @Suppress("DEPRECATION")
        vibrator.vibrate(if (kind == "tap" || kind == "toggle") 10L else 24L)
      }
    }
  }
}
