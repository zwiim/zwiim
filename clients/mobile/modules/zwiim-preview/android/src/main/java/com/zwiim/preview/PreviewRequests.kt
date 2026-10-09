package com.zwiim.preview

import java.util.UUID
import java.util.concurrent.CompletableFuture
import java.util.concurrent.ConcurrentHashMap

data class PreviewAnswer(val status: Int, val mimeType: String?, val file: String?, val location: String?, val text: String?)

/** Scheme requests waiting for JS, by id, across all preview views. */
object PreviewRequests {
  private val pending = ConcurrentHashMap<String, CompletableFuture<PreviewAnswer>>()

  fun add(): Pair<String, CompletableFuture<PreviewAnswer>> {
    val id = UUID.randomUUID().toString()
    val future = CompletableFuture<PreviewAnswer>()
    pending[id] = future
    return id to future
  }

  fun drop(id: String) { pending.remove(id) }

  fun respond(id: String, answer: PreviewAnswer) {
    val future = pending.remove(id)
    if (future == null) {
      answer.file?.let { java.io.File(android.net.Uri.parse(it).path ?: return).delete() }
      return
    }
    future.complete(answer)
  }
}
