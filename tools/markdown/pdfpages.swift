import Foundation
import PDFKit
import AppKit

let args = CommandLine.arguments
let pdf = PDFDocument(url: URL(fileURLWithPath: args[1]))!
let outPrefix = args[2]
let scale: CGFloat = args.count > 3 ? CGFloat(Double(args[3])!) : 1.5
print("pages: \(pdf.pageCount)")
for i in 0..<pdf.pageCount {
    let page = pdf.page(at: i)!
    let bounds = page.bounds(for: .mediaBox)
    let size = NSSize(width: bounds.width * scale, height: bounds.height * scale)
    let image = NSImage(size: size)
    image.lockFocus()
    NSColor.white.setFill()
    NSRect(origin: .zero, size: size).fill()
    let ctx = NSGraphicsContext.current!.cgContext
    ctx.scaleBy(x: scale, y: scale)
    page.draw(with: .mediaBox, to: ctx)
    image.unlockFocus()
    let rep = NSBitmapImageRep(data: image.tiffRepresentation!)!
    let png = rep.representation(using: .png, properties: [:])!
    try! png.write(to: URL(fileURLWithPath: "\(outPrefix)-\(i + 1).png"))
}
