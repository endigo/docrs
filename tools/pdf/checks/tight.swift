// Page 1: Core Text lines 20 pt apart. Page 2: a paragraph at 0.8 line height.
import AppKit
import CoreText

let url = URL(fileURLWithPath: CommandLine.arguments[1]) as CFURL
var box = CGRect(x: 0, y: 0, width: 595.28, height: 841.89)
let ctx = CGContext(url, mediaBox: &box, nil)!

ctx.beginPDFPage(nil)
for i in 0..<5 {
    let font = CTFontCreateWithName((i % 2 == 0 ? "Times New Roman" : "Arial") as CFString, 12, nil)
    let text = NSAttributedString(string: "Line \(i + 1): quick (brown) fox, jumps; “quotes” fi", attributes: [.font: font])
    ctx.textPosition = CGPoint(x: 72, y: 841.89 - 100 - Double(i) * 20)
    CTLineDraw(CTLineCreateWithAttributedString(text), ctx)
}
ctx.endPDFPage()

ctx.beginPDFPage(nil)
let style = NSMutableParagraphStyle()
style.lineHeightMultiple = 0.8
let words = Array(repeating: "Tight lines overlap their neighbours.", count: 12).joined(separator: " ")
let text = NSAttributedString(string: words, attributes: [.font: NSFont(name: "Arial", size: 12)!, .paragraphStyle: style])
let frame = CTFramesetterCreateFrame(
    CTFramesetterCreateWithAttributedString(text), CFRange(location: 0, length: 0),
    CGPath(rect: CGRect(x: 72, y: 500, width: 300, height: 241.89), transform: nil), nil)
CTFrameDraw(frame, ctx)
ctx.endPDFPage()
ctx.closePDF()
