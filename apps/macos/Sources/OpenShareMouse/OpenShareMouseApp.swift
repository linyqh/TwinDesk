import SwiftUI

@main
struct OpenShareMouseApp: App {
    @State private var model = AppModel()

    var body: some Scene {
        Window("OpenShareMouse", id: "main") {
            MainView()
                .environment(model)
        }
        .windowResizability(.contentSize)

        MenuBarExtra {
            MenuContent()
                .environment(model)
        } label: {
            Image(systemName: model.menuBarSymbol)
        }
    }
}
