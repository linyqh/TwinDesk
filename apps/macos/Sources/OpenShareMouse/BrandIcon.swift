import SwiftUI

/// Shared artwork for the window and menu bar; also used to build TwinDesk.icns.
struct BrandIcon: View {
    var size: CGFloat

    var body: some View {
        Image("TwinDesk", bundle: .module)
            .renderingMode(.original)
            .resizable()
            .scaledToFit()
            .frame(width: size, height: size)
            .accessibilityHidden(true)
    }
}
