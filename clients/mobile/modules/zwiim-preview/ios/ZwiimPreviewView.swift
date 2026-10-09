import ExpoModulesCore
import UIKit
import WebKit

/// Pending scheme requests, by id, across all preview views.
final class PreviewRequests {
  static let shared = PreviewRequests()
  private var tasks: [String: (WKURLSchemeTask, URL)] = [:]
  /// Tasks WebKit has not stopped. Answering a stopped task raises an
  /// Objective-C exception, which Swift cannot catch.
  private var live = Set<ObjectIdentifier>()
  private let lock = NSLock()

  func add(_ task: WKURLSchemeTask, url: URL) -> String {
    let id = UUID().uuidString
    lock.lock(); tasks[id] = (task, url); live.insert(ObjectIdentifier(task)); lock.unlock()
    return id
  }

  func drop(_ task: WKURLSchemeTask) {
    lock.lock(); tasks = tasks.filter { $0.value.0 !== task }; live.remove(ObjectIdentifier(task)); lock.unlock()
  }

  func dropAll(for handler: PreviewSchemeHandler) {
    lock.lock()
    tasks = tasks.filter { !handler.owns($0.value.0) }
    live = live.filter { id in !handler.owns(id) }
    lock.unlock()
  }

  private func isLive(_ task: WKURLSchemeTask) -> Bool {
    lock.lock(); defer { lock.unlock() }
    return live.contains(ObjectIdentifier(task))
  }

  func respond(_ id: String, _ r: PreviewResponse) {
    lock.lock(); let entry = tasks.removeValue(forKey: id); lock.unlock()
    guard let (task, url) = entry else {
      if let f = r.file, let fileURL = URL(string: f) { try? FileManager.default.removeItem(at: fileURL) }
      return
    }
    // The file is read off the main thread; each call into the task is made
    // on it, and only while WebKit still wants the answer.
    DispatchQueue.global(qos: .userInitiated).async { self.answer(task, url: url, r) }
  }

  /// Runs `body` on the main thread if the task is still live; false once WebKit stopped it.
  private func send(_ task: WKURLSchemeTask, _ body: (WKURLSchemeTask) -> Void) -> Bool {
    DispatchQueue.main.sync {
      guard isLive(task) else { return false }
      body(task)
      return true
    }
  }

  private func answer(_ task: WKURLSchemeTask, url: URL, _ r: PreviewResponse) {
    let fileURL = r.file.flatMap { URL(string: $0) }
    defer { if let f = fileURL { try? FileManager.default.removeItem(at: f) } }
    var headers = ["Cache-Control": "no-store"]
    if let t = r.mime_type ?? (r.text != nil ? "text/plain; charset=utf-8" : nil) { headers["Content-Type"] = t }
    if let l = r.location { headers["Location"] = l }
    guard let response = HTTPURLResponse(url: url, statusCode: r.status, httpVersion: "HTTP/1.1", headerFields: headers),
      send(task, { $0.didReceive(response) })
    else { return }
    do {
      if let f = fileURL {
        let handle = try FileHandle(forReadingFrom: f)
        defer { try? handle.close() }
        while let chunk = try handle.read(upToCount: 1 << 20), !chunk.isEmpty {
          guard send(task, { $0.didReceive(chunk) }) else { return }
        }
      } else if let t = r.text {
        guard send(task, { $0.didReceive(Data(t.utf8)) }) else { return }
      }
      _ = send(task, { $0.didFinish() })
    } catch {
      _ = send(task, { $0.didFailWithError(error) })
    }
    lock.lock(); live.remove(ObjectIdentifier(task)); lock.unlock()
  }
}

final class PreviewSchemeHandler: NSObject, WKURLSchemeHandler {
  let previewId: String
  let onRequest: (String, String, String) -> Void
  private var live = NSHashTable<AnyObject>.weakObjects()

  init(previewId: String, onRequest: @escaping (String, String, String) -> Void) {
    self.previewId = previewId
    self.onRequest = onRequest
  }

  func owns(_ task: WKURLSchemeTask) -> Bool { live.contains(task) }
  func owns(_ id: ObjectIdentifier) -> Bool { live.allObjects.contains { ObjectIdentifier($0) == id } }

  func webView(_ webView: WKWebView, start task: WKURLSchemeTask) {
    guard let url = task.request.url, url.host == previewId else {
      task.didReceive(HTTPURLResponse(url: task.request.url!, statusCode: 404, httpVersion: "HTTP/1.1", headerFields: ["Content-Type": "text/plain; charset=utf-8"])!)
      task.didReceive(Data("Not found".utf8))
      return task.didFinish()
    }
    live.add(task)
    let id = PreviewRequests.shared.add(task, url: url)
    var path = url.path.isEmpty ? "/" : (URLComponents(url: url, resolvingAgainstBaseURL: false)?.percentEncodedPath ?? "/")
    if let q = url.query { path += "?" + q }
    onRequest(id, task.request.httpMethod ?? "GET", path)
  }

  func webView(_ webView: WKWebView, stop task: WKURLSchemeTask) {
    live.remove(task)
    PreviewRequests.shared.drop(task)
  }
}

class ZwiimPreviewView: ExpoView, WKNavigationDelegate, WKUIDelegate {
  let onRequest = EventDispatcher()
  let onFailed = EventDispatcher()
  private var webView: WKWebView?
  private var handler: PreviewSchemeHandler?
  private var loaded: String?
  private var previewId = ""

  func load(_ source: [String: Any]) {
    guard let kind = source["kind"] as? String, let id = source["previewId"] as? String, loaded != id else { return }
    loaded = id
    previewId = id
    webView?.removeFromSuperview()
    if let h = handler { PreviewRequests.shared.dropAll(for: h) }

    let config = WKWebViewConfiguration()
    config.websiteDataStore = .nonPersistent()
    config.allowsInlineMediaPlayback = true
    if kind == "html" {
      let h = PreviewSchemeHandler(previewId: id) { [weak self] requestId, method, path in
        DispatchQueue.main.async { self?.onRequest(["requestId": requestId, "method": method, "path": path]) }
      }
      handler = h
      config.setURLSchemeHandler(h, forURLScheme: "zwiim-preview")
    }
    let web = WKWebView(frame: bounds, configuration: config)
    web.autoresizingMask = [.flexibleWidth, .flexibleHeight]
    web.navigationDelegate = self
    web.uiDelegate = self
    addSubview(web)
    webView = web

    if kind == "html", let start = source["startPath"] as? String, let url = URL(string: "zwiim-preview://\(id)\(start)") {
      web.load(URLRequest(url: url))
    } else if let file = source["file"] as? String, let url = URL(string: file) {
      web.loadFileURL(url, allowingReadAccessTo: url.deletingLastPathComponent())
    }
  }

  private func isOwn(_ url: URL) -> Bool {
    (url.scheme == "zwiim-preview" && url.host == previewId) || url.isFileURL || url.scheme == "about" || url.scheme == "blob" || url.scheme == "data"
  }

  func webView(_ webView: WKWebView, decidePolicyFor action: WKNavigationAction, decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
    guard let url = action.request.url else { return decisionHandler(.cancel) }
    let mainFrame = action.targetFrame?.isMainFrame ?? true
    if mainFrame && !isOwn(url) {
      if ["http", "https", "mailto"].contains(url.scheme ?? "") { UIApplication.shared.open(url) }
      return decisionHandler(.cancel)
    }
    decisionHandler(.allow)
  }

  func webView(_ webView: WKWebView, createWebViewWith configuration: WKWebViewConfiguration, for action: WKNavigationAction, windowFeatures: WKWindowFeatures) -> WKWebView? {
    if let url = action.request.url, ["http", "https", "mailto"].contains(url.scheme ?? "") { UIApplication.shared.open(url) }
    return nil
  }

  func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
    onFailed(["message": error.localizedDescription])
  }

  func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
    onFailed(["message": error.localizedDescription])
  }

  deinit {
    if let h = handler { PreviewRequests.shared.dropAll(for: h) }
  }
}
