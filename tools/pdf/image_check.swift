// Compares each image test PDF, rendered by PDFKit at one pixel per point,
// with its source image decoded by ImageIO, both over white in sRGB.
//
//   DOCRS_IMAGE_CASES=DIR cargo test -p docrs --test pdf_images
//   swift tools/pdf/image_check.swift DIR [TOLERANCE]
//
// Prints one line per case: the largest channel difference, the mean, and
// the share of pixels differing by more than TOLERANCE (default 2). Exits 1
// when a case other than a profiled or CMYK one, whose colours are
// expected to move, has such pixels.

import AppKit
import Foundation
import ImageIO
import PDFKit

let dir = URL(fileURLWithPath: CommandLine.arguments[1])
let tolerance = CommandLine.arguments.count > 2 ? Int(CommandLine.arguments[2])! : 2
let srgb = CGColorSpace(name: CGColorSpace.sRGB)!

func canvas(_ width: Int, _ height: Int) -> CGContext {
    let context = CGContext(
        data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: width * 4,
        space: srgb, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    context.setFillColor(CGColor(red: 1, green: 1, blue: 1, alpha: 1))
    context.fill(CGRect(x: 0, y: 0, width: width, height: height))
    context.interpolationQuality = .none
    return context
}

func pixels(_ context: CGContext) -> [UInt8] {
    let data = context.data!.bindMemory(to: UInt8.self, capacity: context.height * context.bytesPerRow)
    return Array(UnsafeBufferPointer(start: data, count: context.height * context.bytesPerRow))
}

var failed = false
let files = try FileManager.default.contentsOfDirectory(at: dir, includingPropertiesForKeys: nil)
for pdfURL in files.filter({ $0.pathExtension == "pdf" }).sorted(by: { $0.path < $1.path }) {
    let stem = pdfURL.deletingPathExtension().lastPathComponent
    guard let source = files.first(where: {
        $0.deletingPathExtension().lastPathComponent == stem && $0.pathExtension != "pdf"
    }) else { continue }
    let document = PDFDocument(url: pdfURL)!
    let page = document.page(at: 0)!
    let bounds = page.bounds(for: .mediaBox)
    let (width, height) = (Int(bounds.width), Int(bounds.height))
    let rendered = canvas(width, height)
    page.draw(with: .mediaBox, to: rendered)
    let decoded = canvas(width, height)
    let image = CGImageSourceCreateImageAtIndex(CGImageSourceCreateWithURL(source as CFURL, nil)!, 0, nil)!
    decoded.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
    let (a, b) = (pixels(rendered), pixels(decoded))
    var (largest, total, over) = (0, 0, 0)
    for pixel in 0..<(width * height) {
        var worst = 0
        for channel in 0..<3 {
            worst = max(worst, abs(Int(a[pixel * 4 + channel]) - Int(b[pixel * 4 + channel])))
        }
        largest = max(largest, worst)
        total += worst
        if worst > tolerance { over += 1 }
    }
    let expectedToMove = stem.contains("cmyk") || stem.contains("p3") || stem.contains("profiled")
    let share = Double(over) / Double(width * height) * 100
    let verdict = over == 0 ? "ok" : expectedToMove ? "differs (expected)" : "DIFFERS"
    if over > 0 && !expectedToMove { failed = true }
    let size = "\(width)×\(height)"
    print(
        stem.padding(toLength: 24, withPad: " ", startingAt: 0)
            + size.padding(toLength: 9, withPad: " ", startingAt: 0)
            + String(
                format: "max %3d  mean %5.2f  >%d: %5.1f%%  ", largest,
                Double(total) / Double(width * height), tolerance, share) + verdict)
}
exit(failed ? 1 : 0)
