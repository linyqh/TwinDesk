import SwiftUI

/// Window artwork; also used to build TwinDesk.icns.
struct BrandIcon: View {
    var size: CGFloat

    // SwiftUI's Image(_:bundle:) only searches asset catalogs, not loose PNGs.
    // Not Bundle.module: Xcode 26's generated accessor skips Contents/Resources and traps in the .app.
    private static let artwork: NSImage = {
        let bundle = [Bundle.main.resourceURL, Bundle.main.bundleURL]
            .compactMap { $0?.appendingPathComponent("TwinDesk_OpenShareMouse.bundle") }
            .lazy.compactMap(Bundle.init(url:)).first
        return bundle?.url(forResource: "TwinDesk", withExtension: "png")
            .flatMap(NSImage.init(contentsOf:)) ?? NSImage()
    }()

    var body: some View {
        Image(nsImage: Self.artwork)
            .resizable()
            .scaledToFit()
            .frame(width: size, height: size)
            .accessibilityHidden(true)
    }
}
