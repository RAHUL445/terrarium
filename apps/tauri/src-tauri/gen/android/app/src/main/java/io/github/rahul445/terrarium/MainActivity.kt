package io.github.rahul445.terrarium

import android.os.Bundle
import android.view.View
import androidx.activity.enableEdgeToEdge
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    // Edge-to-edge leaves the WebView behind the IME even with adjustResize.
    // Resize its native content container so CSS/visualViewport see the usable
    // area. System-bar safe areas remain owned by the existing web UI.
    val content = findViewById<View>(android.R.id.content)
    val left = content.paddingLeft
    val top = content.paddingTop
    val right = content.paddingRight
    val bottom = content.paddingBottom
    ViewCompat.setOnApplyWindowInsetsListener(content) { view, insets ->
      val keyboard = insets.getInsets(WindowInsetsCompat.Type.ime()).bottom
      view.setPadding(left, top, right, bottom + keyboard)
      insets
    }
    ViewCompat.requestApplyInsets(content)
  }
}
