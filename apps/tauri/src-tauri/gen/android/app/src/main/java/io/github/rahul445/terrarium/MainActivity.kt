package io.github.rahul445.terrarium

import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.view.View
import androidx.activity.enableEdgeToEdge
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    intent = shareAsLink(intent)
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

  override fun onNewIntent(intent: Intent) {
    val link = shareAsLink(intent)
    setIntent(link)
    super.onNewIntent(link)
  }

  /** Text shared from another app becomes terrarium://share?…, which the
   *  deep-link plugin delivers like any other link. */
  private fun shareAsLink(intent: Intent): Intent {
    if (intent.action != Intent.ACTION_SEND) return intent
    val type = intent.type ?: return intent
    if (type != "text/plain" && type != "text/uri-list") return intent
    val text = intent.getStringExtra(Intent.EXTRA_TEXT)?.take(10000) ?: return intent
    val builder = Uri.Builder()
      .scheme("terrarium")
      .authority("share")
      .appendQueryParameter("text", text)
      .appendQueryParameter("type", type)
    intent.getStringExtra(Intent.EXTRA_SUBJECT)
      ?.take(200)
      ?.let { builder.appendQueryParameter("subject", it) }
    return Intent(Intent.ACTION_VIEW, builder.build())
  }
}
