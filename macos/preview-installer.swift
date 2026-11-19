import AppKit

let args = CommandLine.arguments
let art = NSImage(contentsOfFile: args[1])!
let canvas = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: 1536, pixelsHigh: 1024,
    bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
    colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
let labels: [(name: String, x: CGFloat, width: CGFloat)] = [
    ("剪栈", 790, 124),
    ("Applications", 1320, 228),
]
let iconY: CGFloat = 1024 - 552
let labelY: CGFloat = 1024 - 682

NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: canvas)
NSGraphicsContext.current?.imageInterpolation = .high
art.draw(in: NSRect(x: 0, y: 0, width: 1536, height: 1024))

// Finder draws its own black text over picture backgrounds. Keep the plates
// part of the artwork so the installed volume and its layout preview agree.
let highlight = NSColor(srgbRed: 0.93, green: 0.96, blue: 0.99, alpha: 1)
let base = NSColor(srgbRed: 0.80, green: 0.86, blue: 0.92, alpha: 1)
let gradient = NSGradient(starting: highlight, ending: base)!
for label in labels {
    let plate = NSRect(x: label.x - label.width / 2, y: labelY - 25,
                       width: label.width, height: 50)
    let path = NSBezierPath(roundedRect: plate, xRadius: 25, yRadius: 25)
    NSGraphicsContext.saveGraphicsState()
    let shadow = NSShadow()
    shadow.shadowColor = NSColor.black.withAlphaComponent(0.24)
    shadow.shadowBlurRadius = 10
    shadow.shadowOffset = NSSize(width: 0, height: -3)
    shadow.set()
    base.setFill()
    path.fill()
    NSGraphicsContext.restoreGraphicsState()
    gradient.draw(in: path, angle: 270)
    NSColor.white.withAlphaComponent(0.48).setStroke()
    path.lineWidth = 1.5
    path.stroke()
}

if args[2] != "--background" {
    let appIcon = NSWorkspace.shared.icon(forFile: args[2])
    let coreIcons = "/System/Library/CoreServices/CoreTypes.bundle/Contents/Resources"
    let folder = NSImage(contentsOfFile: coreIcons + "/ApplicationsFolderIcon.icns")!
    let alias = NSImage(contentsOfFile: coreIcons + "/AliasBadgeIcon.icns")
    for (index, image) in [appIcon, folder].enumerated() {
        let label = labels[index]
        let bounds = NSRect(x: label.x - 96, y: iconY - 96, width: 192, height: 192)
        image.draw(in: bounds)
        if index == 1 { alias?.draw(in: NSRect(x: bounds.minX, y: bounds.minY, width: 44, height: 44)) }
        let text = NSAttributedString(string: label.name, attributes: [
            .font: NSFont.systemFont(ofSize: 26),
            .foregroundColor: NSColor.black,
        ])
        let size = text.size()
        text.draw(at: NSPoint(x: label.x - size.width / 2, y: labelY - size.height / 2))
    }
}
NSGraphicsContext.restoreGraphicsState()
try canvas.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: args[3]))
