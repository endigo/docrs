// Usage: rasterdiff OURS.pdf WORD.pdf [DIFF_PREFIX] (tools/pdf/README.md).
// Renders each page of both PDFs at 144 dpi with PDFKit and prints, per
// page, the share of inked pixels (darker than 191 of 255 in either render)
// whose grey levels differ by more than 64 of 255. A different page count
// or page size is reported and exits 1. With DIFF_PREFIX, writes
// DIFF_PREFIX-N.png: ours in red, Word's in blue, both in black.
import AppKit
import Foundation
import PDFKit

let scale: CGFloat = 2
let tolerance = 64

func grey(_ page: PDFPage, _ size: CGSize) -> [UInt8] {
    let width = Int(size.width), height = Int(size.height)
    var pixels = [UInt8](repeating: 255, count: width * height)
    pixels.withUnsafeMutableBytes { buffer in
        let context = CGContext(
            data: buffer.baseAddress, width: width, height: height, bitsPerComponent: 8,
            bytesPerRow: width, space: CGColorSpaceCreateDeviceGray(),
            bitmapInfo: CGImageAlphaInfo.none.rawValue)!
        context.setFillColor(gray: 1, alpha: 1)
        context.fill(CGRect(x: 0, y: 0, width: width, height: height))
        context.scaleBy(x: scale, y: scale)
        page.draw(with: .mediaBox, to: context)
    }
    return pixels
}

let args = CommandLine.arguments
guard args.count >= 3,
    let ours = PDFDocument(url: URL(fileURLWithPath: args[1])),
    let word = PDFDocument(url: URL(fileURLWithPath: args[2]))
else {
    FileHandle.standardError.write("usage: rasterdiff OURS.pdf WORD.pdf [DIFF_PREFIX]\n".data(using: .utf8)!)
    exit(2)
}
var mismatch = false
print("pages\t\(ours.pageCount)\t\(word.pageCount)")
if ours.pageCount != word.pageCount {
    print("mismatch\tpage count")
    mismatch = true
}
for index in 0..<min(ours.pageCount, word.pageCount) {
    let a = ours.page(at: index)!, b = word.page(at: index)!
    let bounds = a.bounds(for: .mediaBox)
    let other = b.bounds(for: .mediaBox)
    if abs(bounds.width - other.width) > 0.5 || abs(bounds.height - other.height) > 0.5 {
        print("mismatch\tpage \(index + 1) size \(bounds.size) against \(other.size)")
        mismatch = true
        continue
    }
    let size = CGSize(width: bounds.width * scale, height: bounds.height * scale)
    let (x, y) = (grey(a, size), grey(b, size))
    let inked = zip(x, y).filter { Int(min($0, $1)) < 255 - tolerance }.count
    let differing = zip(x, y).filter { abs(Int($0) - Int($1)) > tolerance }.count
    print(String(format: "page\t%d\t%.4f", index + 1, Double(differing) / Double(max(inked, 1))))
    if args.count > 3 {
        let width = Int(size.width), height = Int(size.height)
        let image = NSBitmapImageRep(
            bitmapDataPlanes: nil, pixelsWide: width, pixelsHigh: height, bitsPerSample: 8,
            samplesPerPixel: 3, hasAlpha: false, isPlanar: false, colorSpaceName: .deviceRGB,
            bytesPerRow: width * 3, bitsPerPixel: 24)!
        let data = image.bitmapData!
        for at in 0..<(width * height) {
            let (inkA, inkB) = (x[at] < 128, y[at] < 128)
            data[at * 3] = inkB ? 0 : 255
            data[at * 3 + 1] = inkA || inkB ? 0 : 255
            data[at * 3 + 2] = inkA ? 0 : 255
        }
        try! image.representation(using: .png, properties: [:])!
            .write(to: URL(fileURLWithPath: "\(args[3])-\(index + 1).png"))
    }
}
if mismatch { exit(1) }
