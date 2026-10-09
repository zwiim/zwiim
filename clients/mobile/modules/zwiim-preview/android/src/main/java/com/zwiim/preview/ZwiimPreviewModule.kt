package com.zwiim.preview

import expo.modules.kotlin.modules.Module
import expo.modules.kotlin.modules.ModuleDefinition
import expo.modules.kotlin.records.Field
import expo.modules.kotlin.records.Record

class PreviewResponse : Record {
  @Field val status: Int = 502
  @Field val mime_type: String? = null
  @Field val file: String? = null
  @Field val location: String? = null
  @Field val text: String? = null
}

class ZwiimPreviewModule : Module() {
  override fun definition() = ModuleDefinition {
    Name("ZwiimPreview")

    AsyncFunction("respondPreview") { requestId: String, r: PreviewResponse ->
      PreviewRequests.respond(requestId, PreviewAnswer(r.status, r.mime_type, r.file, r.location, r.text))
    }

    View(ZwiimPreviewView::class) {
      Events("onRequest", "onFailed")
      Prop("source") { view: ZwiimPreviewView, source: Map<String, Any?> -> view.load(source) }
    }
  }
}
