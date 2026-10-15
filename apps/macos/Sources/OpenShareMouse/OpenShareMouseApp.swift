import SwiftUI

@main
struct TwinDeskApp: App {
    @State private var model = AppModel()

    var body: some Scene {
        Window("同桌 · TwinDesk", id: "main") {
            MainView()
                .environment(model)
        }
        .windowResizability(.contentSize)

        MenuBarExtra {
            MenuContent()
                .environment(model)
        } label: {
            HStack(spacing: 3) {
                BrandIcon(size: 22)
                Image(systemName: model.menuBarSymbol)
                    .font(.system(size: 10))
            }
            .accessibilityLabel("同桌 · TwinDesk，\(model.linkTitle)，\(model.roleTitle)")
        }
    }
}
