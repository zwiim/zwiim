import ExpoModulesCore
import Network

/// TCP connections to a host on the local network (Network.framework).
/// Every connection is touched only on `queue`.
public class ZwiimLanModule: Module {
  private let queue = DispatchQueue(label: "dev.zwiim.lan")
  private var connections: [String: NWConnection] = [:]

  public func definition() -> ModuleDefinition {
    Name("ZwiimLan")

    Events("onData", "onClose")

    AsyncFunction("connect") { (id: String, address: String, port: Int, timeoutMs: Int, promise: Promise) in
      guard let nwPort = NWEndpoint.Port(rawValue: UInt16(clamping: port)), port > 0 else {
        promise.reject("ERR_ZWIIM_LAN_CONNECT", "Invalid port")
        return
      }
      let tcp = NWProtocolTCP.Options()
      tcp.noDelay = true
      let connection = NWConnection(
        host: NWEndpoint.Host(address), port: nwPort, using: NWParameters(tls: nil, tcp: tcp))
      var settled = false
      let fail = { (message: String) in
        if settled { return }
        settled = true
        connection.stateUpdateHandler = nil
        connection.cancel()
        promise.reject("ERR_ZWIIM_LAN_CONNECT", message)
      }
      connection.stateUpdateHandler = { [weak self] state in
        guard let self else { return }
        switch state {
        case .ready:
          if settled { return }
          settled = true
          self.connections[id] = connection
          connection.stateUpdateHandler = { [weak self] state in
            switch state {
            case .failed(let error): self?.finish(id, reason: error.localizedDescription)
            case .cancelled: self?.finish(id, reason: "closed")
            default: break
            }
          }
          self.receive(id, connection)
          promise.resolve()
        case .waiting(let error), .failed(let error):
          // Refused, unreachable, or local network access denied: no answer
          // is coming.
          fail(error.localizedDescription)
        case .cancelled:
          fail("cancelled")
        default:
          break
        }
      }
      connection.start(queue: queue)
      queue.asyncAfter(deadline: .now() + .milliseconds(max(timeoutMs, 1))) {
        fail("Timed out")
      }
    }

    Function("send") { (id: String, data: String) in
      guard let bytes = Data(base64Encoded: data) else { return }
      self.queue.async {
        guard let connection = self.connections[id] else { return }
        connection.send(content: bytes, completion: .contentProcessed { [weak self] error in
          if let error { self?.finish(id, reason: error.localizedDescription) }
        })
      }
    }

    Function("close") { (id: String) in
      self.queue.async {
        self.connections.removeValue(forKey: id)?.cancel()
      }
    }

    OnDestroy {
      self.queue.sync {
        for connection in self.connections.values { connection.cancel() }
        self.connections.removeAll()
      }
    }
  }

  private func receive(_ id: String, _ connection: NWConnection) {
    connection.receive(minimumIncompleteLength: 1, maximumLength: 65536) { [weak self] data, _, complete, error in
      guard let self else { return }
      if let data, !data.isEmpty, self.connections[id] != nil {
        self.sendEvent("onData", ["id": id, "data": data.base64EncodedString()])
      }
      if let error {
        self.finish(id, reason: error.localizedDescription)
      } else if complete {
        self.finish(id, reason: "closed by host")
      } else {
        self.receive(id, connection)
      }
    }
  }

  /// On `queue`: forget `id` and report its end, once.
  private func finish(_ id: String, reason: String) {
    guard let connection = connections.removeValue(forKey: id) else { return }
    connection.stateUpdateHandler = nil
    connection.cancel()
    sendEvent("onClose", ["id": id, "reason": reason])
  }
}
