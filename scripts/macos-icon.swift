import AppKit
import Foundation

func fail(_ message: String) -> Never {
    fputs("macos-icon: \(message)\n", stderr)
    exit(1)
}

guard CommandLine.arguments.count == 3 else {
    fail("usage: macos-icon.swift INPUT OUTPUT")
}
guard let image = NSImage(contentsOfFile: CommandLine.arguments[1]) else {
    fail("cannot load input image")
}
var proposedRect = CGRect(origin: .zero, size: image.size)
guard let cgImage = image.cgImage(forProposedRect: &proposedRect, context: nil, hints: nil) else {
    fail("cannot decode input image")
}
let source = NSBitmapImageRep(cgImage: cgImage)
let width = source.pixelsWide
let height = source.pixelsHigh
guard let bitmap = NSBitmapImageRep(
    bitmapDataPlanes: nil,
    pixelsWide: width,
    pixelsHigh: height,
    bitsPerSample: 8,
    samplesPerPixel: 4,
    hasAlpha: true,
    isPlanar: false,
    colorSpaceName: .deviceRGB,
    bytesPerRow: 0,
    bitsPerPixel: 0
) else {
    fail("cannot allocate output image")
}
let artworkScale = 0.84
let artworkWidth = Double(width) * artworkScale
let artworkHeight = Double(height) * artworkScale
let marginX = (Double(width) - artworkWidth) / 2
let marginY = (Double(height) - artworkHeight) / 2
let radius = min(artworkWidth, artworkHeight) * 0.18
let halfWidth = artworkWidth / 2
let halfHeight = artworkHeight / 2
let transparent = NSColor(calibratedRed: 0, green: 0, blue: 0, alpha: 0)

for y in 0..<height {
    let localY = Double(y) + 0.5 - marginY
    for x in 0..<width {
        let localX = Double(x) + 0.5 - marginX
        guard localX >= 0, localX < artworkWidth, localY >= 0, localY < artworkHeight else {
            bitmap.setColor(transparent, atX: x, y: y)
            continue
        }
        let sourceX = min(width - 1, Int(localX / artworkWidth * Double(width)))
        let sourceY = min(height - 1, Int(localY / artworkHeight * Double(height)))
        guard let color = source.colorAt(x: sourceX, y: sourceY)?.usingColorSpace(.deviceRGB) else {
            fail("cannot read input pixel")
        }
        let r = color.redComponent
        let g = color.greenComponent
        let b = color.blueComponent
        let maxChannel = max(r, g, b)
        let boosted = maxChannel < 0.48
        let red = boosted ? min(1, r * 1.34 + 0.035) : r
        let green = boosted ? min(1, g * 1.36 + 0.05) : g
        let blue = boosted ? min(1, b * 1.4 + 0.065) : b

        let qx = abs(localX - halfWidth) - (halfWidth - radius)
        let qy = abs(localY - halfHeight) - (halfHeight - radius)
        let outsideX = max(qx, 0)
        let outsideY = max(qy, 0)
        let distance = hypot(outsideX, outsideY) + min(max(qx, qy), 0) - radius
        let coverage = min(1, max(0, 0.5 - distance))
        let output = NSColor(
            calibratedRed: red,
            green: green,
            blue: blue,
            alpha: color.alphaComponent * coverage
        )
        bitmap.setColor(output, atX: x, y: y)
    }
}

guard let png = bitmap.representation(using: .png, properties: [:]) else {
    fail("cannot encode output image")
}
do {
    try png.write(to: URL(fileURLWithPath: CommandLine.arguments[2]), options: .atomic)
} catch {
    fail("cannot write output image: \(error)")
}
