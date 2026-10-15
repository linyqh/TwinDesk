import SwiftUI

/// Window artwork; also used to build TwinDesk.icns.
struct BrandIcon: View {
    var size: CGFloat

    // SwiftUI's Image(_:bundle:) only searches asset catalogs, not loose PNGs.
    private static let artwork = Bundle.module.url(forResource: "TwinDesk", withExtension: "png")
        .flatMap(NSImage.init(contentsOf:)) ?? NSImage()

    var body: some View {
        Image(nsImage: Self.artwork)
            .resizable()
            .scaledToFit()
            .frame(width: size, height: size)
            .accessibilityHidden(true)
    }
}
