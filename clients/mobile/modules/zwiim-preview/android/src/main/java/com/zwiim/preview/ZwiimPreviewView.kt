package com.zwiim.preview

import android.annotation.SuppressLint
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Message
import android.webkit.*
import expo.modules.kotlin.AppContext
import expo.modules.kotlin.viewevent.EventDispatcher
import expo.modules.kotlin.views.ExpoView
import java.io.File
import java.io.FileInputStream
import java.io.FilterInputStream
import java.io.InputStream
import java.util.concurrent.TimeUnit

private const val RENDERER_HOST = "renderer.zwiim-preview.invalid"
private const val WAIT_SECONDS = 60L

class ZwiimPreviewView(context: Context, appContext: AppContext) : ExpoView(context, appContext) {
  private val onRequest by EventDispatcher()
  private val onFailed by EventDispatcher()
  private var web: WebView? = null
  private var previewId = ""
  private var htmlHost = ""
  private var file: File? = null
  private var mimeType = "application/octet-stream"

  @SuppressLint("SetJavaScriptEnabled")
  fun load(source: Map<String, Any?>) {
    val id = source["previewId"] as? String ?: return
    if (id == previewId) return
    previewId = id
    web?.let { removeView(it); it.destroy() }
    val kind = source["kind"] as? String
    htmlHost = if (kind == "html") "$id.zwiim-preview.invalid" else ""
    file = (source["file"] as? String)?.let { Uri.parse(it).path }?.let(::File)
    mimeType = source["mime_type"] as? String ?: mimeType

    val view = WebView(context)
    view.settings.apply {
      javaScriptEnabled = true
      domStorageEnabled = true
      allowFileAccess = false
      allowContentAccess = false
      setSupportMultipleWindows(true)
      builtInZoomControls = true
      displayZoomControls = false
    }
    CookieManager.getInstance().setAcceptThirdPartyCookies(view, false)
    view.webViewClient = Client()
    view.webChromeClient = Chrome()
    addView(view, LayoutParams(LayoutParams.MATCH_PARENT, LayoutParams.MATCH_PARENT))
    web = view

    if (kind == "html") {
      view.loadUrl("https://$htmlHost${source["startPath"] as? String ?: "/"}")
    } else {
      val type = Uri.encode(source["type"] as? String ?: "")
      val name = Uri.encode(source["name"] as? String ?: "")
      view.loadUrl("https://$RENDERER_HOST/index.html?id=$id&kind=$type&name=$name")
    }
  }

  private fun isOwn(uri: Uri) = uri.scheme == "https" && (uri.host == htmlHost && htmlHost.isNotEmpty() || uri.host == RENDERER_HOST)

  private fun openOutside(uri: Uri) {
    if (uri.scheme !in setOf("http", "https", "mailto")) return
    context.startActivity(Intent(Intent.ACTION_VIEW, uri).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
  }

  private fun text(status: Int, body: String) =
    WebResourceResponse("text/plain", "utf-8", status, reason(status), mapOf("Cache-Control" to "no-store"), body.byteInputStream())

  private fun reason(status: Int) = when (status) {
    200 -> "OK"; 206 -> "Partial Content"; 301 -> "Moved Permanently"; 404 -> "Not Found"
    405 -> "Method Not Allowed"; 413 -> "Payload Too Large"; 416 -> "Range Not Satisfiable"; else -> "Bad Gateway"
  }

  /** An HTML preview request: ask JS, wait up to 60 s, stream its cache file. */
  private fun askJs(request: WebResourceRequest): WebResourceResponse {
    if (request.method != "GET" && request.method != "HEAD") return text(405, "Method not allowed")
    val (id, future) = PreviewRequests.add()
    val url = request.url
    val path = (url.encodedPath ?: "/") + (url.encodedQuery?.let { "?$it" } ?: "")
    post { onRequest(mapOf("requestId" to id, "method" to request.method, "path" to path)) }
    val answer = try {
      future.get(WAIT_SECONDS, TimeUnit.SECONDS)
    } catch (e: Exception) {
      PreviewRequests.drop(id)
      return text(502, "The preview did not answer")
    }
    val headers = mutableMapOf("Cache-Control" to "no-store")
    answer.location?.let { headers["Location"] = it }
    val body: InputStream = answer.file?.let { Uri.parse(it).path }?.let { p ->
      val f = File(p)
      object : FilterInputStream(FileInputStream(f)) { override fun close() { super.close(); f.delete() } }
    } ?: (answer.text ?: "").byteInputStream()
    val (mime, charset) = splitType(answer.mimeType ?: if (answer.text != null) "text/plain; charset=utf-8" else "application/octet-stream")
    return WebResourceResponse(mime, charset, answer.status, reason(answer.status), headers, body)
  }

  private fun splitType(type: String): Pair<String, String?> {
    val parts = type.split(";").map { it.trim() }
    val charset = parts.drop(1).firstOrNull { it.startsWith("charset=", true) }?.substringAfter("=")
    return parts[0] to charset
  }

  /** The renderer's own files, and the previewed file with Range support. */
  private fun renderer(request: WebResourceRequest): WebResourceResponse {
    val path = request.url.path ?: "/"
    if (path == "/file/$previewId") {
      val f = file ?: return text(404, "Not found")
      val size = f.length()
      val range = request.requestHeaders.entries.firstOrNull { it.key.equals("Range", true) }?.value
      val m = range?.let { Regex("""bytes=(\d+)-(\d*)""").find(it) }
      val headers = mutableMapOf("Accept-Ranges" to "bytes", "Cache-Control" to "no-store")
      if (m == null) {
        headers["Content-Length"] = size.toString()
        return WebResourceResponse(mimeType, null, 200, "OK", headers, FileInputStream(f))
      }
      val start = m.groupValues[1].toLong()
      val end = m.groupValues[2].toLongOrNull()?.coerceAtMost(size - 1) ?: (size - 1)
      if (start >= size || end < start) return text(416, "Range not satisfiable")
      val stream = FileInputStream(f).apply { skip(start) }
      headers["Content-Range"] = "bytes $start-$end/$size"
      headers["Content-Length"] = (end - start + 1).toString()
      return WebResourceResponse(mimeType, null, 206, "Partial Content", headers, BoundedStream(stream, end - start + 1))
    }
    val asset = path.trimStart('/').ifEmpty { "index.html" }
    if (asset.contains("..")) return text(404, "Not found")
    return try {
      val stream = context.assets.open("zwiim-preview/$asset")
      WebResourceResponse(assetType(asset), null, 200, "OK", mapOf("Cache-Control" to "no-store"), stream)
    } catch (e: Exception) {
      text(404, "Not found")
    }
  }

  private fun assetType(name: String) = when (name.substringAfterLast('.', "")) {
    "html" -> "text/html"; "js", "mjs" -> "text/javascript"; "css" -> "text/css"; "wasm" -> "application/wasm"
    "json" -> "application/json"; "pfb", "ttf" -> "font/ttf"; "bcmap" -> "application/octet-stream"; else -> "application/octet-stream"
  }

  private inner class Client : WebViewClient() {
    override fun shouldInterceptRequest(view: WebView, request: WebResourceRequest): WebResourceResponse? {
      val host = request.url.host ?: return null
      return when {
        htmlHost.isNotEmpty() && host == htmlHost -> askJs(request)
        host == RENDERER_HOST -> renderer(request)
        host.endsWith(".zwiim-preview.invalid") -> text(404, "Not found")
        else -> null // other sites: ordinary network, as on any web page
      }
    }

    override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest): Boolean {
      if (!request.isForMainFrame || isOwn(request.url)) return false
      openOutside(request.url)
      return true
    }

    override fun onReceivedError(view: WebView, request: WebResourceRequest, error: WebResourceError) {
      if (request.isForMainFrame) onFailed(mapOf("message" to error.description.toString()))
    }
  }

  private inner class Chrome : WebChromeClient() {
    // A new window opens in the system browser: catch its first URL, then drop it.
    override fun onCreateWindow(view: WebView, isDialog: Boolean, isUserGesture: Boolean, resultMsg: Message): Boolean {
      val catcher = WebView(context)
      catcher.webViewClient = object : WebViewClient() {
        override fun shouldOverrideUrlLoading(v: WebView, request: WebResourceRequest): Boolean {
          openOutside(request.url)
          v.destroy()
          return true
        }
      }
      (resultMsg.obj as WebView.WebViewTransport).webView = catcher
      resultMsg.sendToTarget()
      return true
    }
  }

  override fun onDetachedFromWindow() {
    super.onDetachedFromWindow()
    web?.destroy()
    web = null
    previewId = ""
  }
}

private class BoundedStream(inner: InputStream, private var left: Long) : FilterInputStream(inner) {
  override fun read(): Int = if (left <= 0) -1 else super.read().also { if (it >= 0) left-- }
  override fun read(b: ByteArray, off: Int, len: Int): Int {
    if (left <= 0) return -1
    val n = super.read(b, off, minOf(len.toLong(), left).toInt())
    if (n > 0) left -= n
    return n
  }
}
