// Keeps the page clear of the phone's bars and keyboard, and the bars in its color.
package dev.cursorbyok.mobile

import android.graphics.Color
import android.os.Bundle
import android.view.View
import android.webkit.JavascriptInterface
import android.webkit.WebView
import androidx.activity.enableEdgeToEdge
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
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

  override fun onWebViewCreate(webView: WebView) {
    webView.addJavascriptInterface(Bars(), "Bars")
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
}
