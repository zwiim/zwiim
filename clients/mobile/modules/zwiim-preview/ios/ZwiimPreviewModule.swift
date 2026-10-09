import ExpoModulesCore

public class ZwiimPreviewModule: Module {
  public func definition() -> ModuleDefinition {
    Name("ZwiimPreview")

    AsyncFunction("respondPreview") { (requestId: String, response: PreviewResponse) in
      PreviewRequests.shared.respond(requestId, response)
    }

    View(ZwiimPreviewView.self) {
      Events("onRequest", "onFailed")
      Prop("source") { (view: ZwiimPreviewView, source: [String: Any]) in
        view.load(source)
      }
    }
  }
}

struct PreviewResponse: Record {
  @Field var status: Int = 502
  @Field var mime_type: String?
  @Field var file: String?
  @Field var location: String?
  @Field var text: String?
}
