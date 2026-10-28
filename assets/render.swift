// Renders an SVG to a square PNG on a transparent background, inset on each side by a fraction
// of the width. Apple's app icon grid insets the icon by 100 of 1024 points.
//
//   swiftc -O assets/render.swift -o /tmp/render
//   /tmp/render assets/jira.svg assets/jira.png 512
//   /tmp/render assets/jira.svg assets/jira-macos.png 1024 0.09765625
import AppKit

let args = CommandLine.arguments
guard args.count >= 4, let px = Int(args[3]) else {
    FileHandle.standardError.write(Data("usage: render SRC.svg DST.png PIXELS [INSET]\n".utf8))
    exit(2)
}
let inset = args.count > 4 ? Double(args[4]) ?? 0 : 0
guard let image = NSImage(contentsOfFile: args[1]) else {
    FileHandle.standardError.write(Data("cannot read \(args[1])\n".utf8))
    exit(1)
}
let rep = NSBitmapImageRep(
    bitmapDataPlanes: nil, pixelsWide: px, pixelsHigh: px, bitsPerSample: 8, samplesPerPixel: 4,
    hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
rep.size = NSSize(width: px, height: px)
NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
NSGraphicsContext.current!.imageInterpolation = .high
let margin = Double(px) * inset
let side = Double(px) - 2 * margin
image.draw(in: NSRect(x: margin, y: margin, width: side, height: side), from: .zero,
           operation: .sourceOver, fraction: 1)
NSGraphicsContext.restoreGraphicsState()
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: args[2]))
