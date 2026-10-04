package com.zwiim.lan

import android.util.Base64
import expo.modules.kotlin.Promise
import expo.modules.kotlin.modules.Module
import expo.modules.kotlin.modules.ModuleDefinition
import java.io.IOException
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.Socket
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors

/** TCP connections to a host on the local network. */
class ZwiimLanModule : Module() {
  private class Connection(val socket: Socket) {
    /** Writes, one at a time in call order. */
    val writer: ExecutorService = Executors.newSingleThreadExecutor()
  }

  private val connections = ConcurrentHashMap<String, Connection>()
  private val readers = Executors.newCachedThreadPool()

  override fun definition() = ModuleDefinition {
    Name("ZwiimLan")

    Events("onData", "onClose")

    AsyncFunction("connect") { id: String, address: String, port: Int, timeoutMs: Int, promise: Promise ->
      readers.execute {
        val socket = Socket()
        try {
          socket.tcpNoDelay = true
          // A literal address: never resolved through DNS.
          socket.connect(InetSocketAddress(InetAddress.getByName(address), port), timeoutMs.coerceAtLeast(1))
        } catch (e: Exception) {
          socket.close()
          promise.reject("ERR_ZWIIM_LAN_CONNECT", e.message ?: "connect failed", e)
          return@execute
        }
        connections[id] = Connection(socket)
        promise.resolve(null)
        read(id, socket)
      }
    }

    Function("send") { id: String, data: String ->
      val connection = connections[id] ?: return@Function
      val bytes = Base64.decode(data, Base64.NO_WRAP)
      connection.writer.execute {
        try {
          connection.socket.getOutputStream().apply {
            write(bytes)
            flush()
          }
        } catch (e: IOException) {
          finish(id, e.message ?: "write failed")
        }
      }
    }

    Function("close") { id: String ->
      connections.remove(id)?.let { shut(it) }
    }

    OnDestroy {
      for (connection in connections.values) shut(connection)
      connections.clear()
      readers.shutdownNow()
    }
  }

  private fun read(id: String, socket: Socket) {
    val buf = ByteArray(65536)
    var reason = "closed by host"
    try {
      val input = socket.getInputStream()
      while (true) {
        val n = input.read(buf)
        if (n < 0) break
        if (!connections.containsKey(id)) return
        sendEvent("onData", mapOf("id" to id, "data" to Base64.encodeToString(buf, 0, n, Base64.NO_WRAP)))
      }
    } catch (e: IOException) {
      reason = e.message ?: "read failed"
    }
    finish(id, reason)
  }

  /** Forget `id` and report its end, once. */
  private fun finish(id: String, reason: String) {
    val connection = connections.remove(id) ?: return
    shut(connection)
    sendEvent("onClose", mapOf("id" to id, "reason" to reason))
  }

  private fun shut(connection: Connection) {
    connection.writer.shutdown()
    try {
      connection.socket.close()
    } catch (_: IOException) {
    }
  }
}
