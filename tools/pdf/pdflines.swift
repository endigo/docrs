// Dump the text lines of each page of a PDF as JSON, in points from the
// page's top-left corner, one line of output per text line:
//   swiftc -O tools/pdf/pdflines.swift -o pdflines && ./pdflines FILE.pdf > lines.json
// Lines, their text and bounds are PDFKit's (`selectionsByLine`). The
// baseline is the y of the text origin (Tm and CTM, from the page's content
// stream) near the line's bounds that most of its text showing operators
// share; null when none is near (text drawn from a form XObject).
import CoreGraphics
import Foundation
import PDFKit

final class TextState {
    var ctm = CGAffineTransform.identity
    var stack: [CGAffineTransform] = []
    var tm = CGAffineTransform.identity
    var tlm = CGAffineTransform.identity
    var leading: CGFloat = 0
    var rise: CGFloat = 0
    var origins: [CGPoint] = []

    func moveLine(_ tx: CGFloat, _ ty: CGFloat) {
        tlm = CGAffineTransform(translationX: tx, y: ty).concatenating(tlm)
        tm = tlm
    }

    func show() {
        origins.append(CGPoint(x: 0, y: rise).applying(tm.concatenating(ctm)))
    }
}

func numbers(_ scanner: CGPDFScannerRef, _ count: Int) -> [CGFloat]? {
    var out = [CGFloat](repeating: 0, count: count)
    for i in (0..<count).reversed() {
        var value: CGPDFReal = 0
        guard CGPDFScannerPopNumber(scanner, &value) else { return nil }
        out[i] = value
    }
    return out
}

func state(_ info: UnsafeMutableRawPointer?) -> TextState {
    Unmanaged<TextState>.fromOpaque(info!).takeUnretainedValue()
}

func textOrigins(_ page: CGPDFPage) -> [CGPoint] {
    let table = CGPDFOperatorTableCreate()!
    CGPDFOperatorTableSetCallback(table, "q") { _, info in
        let s = state(info)
        s.stack.append(s.ctm)
    }
    CGPDFOperatorTableSetCallback(table, "Q") { _, info in
        let s = state(info)
        if let ctm = s.stack.popLast() { s.ctm = ctm }
    }
    CGPDFOperatorTableSetCallback(table, "cm") { scanner, info in
        guard let n = numbers(scanner, 6) else { return }
        let s = state(info)
        s.ctm = CGAffineTransform(a: n[0], b: n[1], c: n[2], d: n[3], tx: n[4], ty: n[5]).concatenating(s.ctm)
    }
    CGPDFOperatorTableSetCallback(table, "BT") { _, info in
        let s = state(info)
        s.tm = .identity
        s.tlm = .identity
    }
    CGPDFOperatorTableSetCallback(table, "Tm") { scanner, info in
        guard let n = numbers(scanner, 6) else { return }
        let s = state(info)
        s.tlm = CGAffineTransform(a: n[0], b: n[1], c: n[2], d: n[3], tx: n[4], ty: n[5])
        s.tm = s.tlm
    }
    CGPDFOperatorTableSetCallback(table, "Td") { scanner, info in
        guard let n = numbers(scanner, 2) else { return }
        state(info).moveLine(n[0], n[1])
    }
    CGPDFOperatorTableSetCallback(table, "TD") { scanner, info in
        guard let n = numbers(scanner, 2) else { return }
        let s = state(info)
        s.leading = -n[1]
        s.moveLine(n[0], n[1])
    }
    CGPDFOperatorTableSetCallback(table, "TL") { scanner, info in
        guard let n = numbers(scanner, 1) else { return }
        state(info).leading = n[0]
    }
    CGPDFOperatorTableSetCallback(table, "Ts") { scanner, info in
        guard let n = numbers(scanner, 1) else { return }
        state(info).rise = n[0]
    }
    CGPDFOperatorTableSetCallback(table, "T*") { _, info in
        let s = state(info)
        s.moveLine(0, -s.leading)
    }
    CGPDFOperatorTableSetCallback(table, "Tj") { _, info in state(info).show() }
    CGPDFOperatorTableSetCallback(table, "TJ") { _, info in state(info).show() }
    CGPDFOperatorTableSetCallback(table, "'") { _, info in
        let s = state(info)
        s.moveLine(0, -s.leading)
        s.show()
    }
    CGPDFOperatorTableSetCallback(table, "\"") { _, info in
        let s = state(info)
        s.moveLine(0, -s.leading)
        s.show()
    }
    let s = TextState()
    let stream = CGPDFContentStreamCreateWithPage(page)
    let scanner = CGPDFScannerCreate(stream, table, Unmanaged.passUnretained(s).toOpaque())
    CGPDFScannerScan(scanner)
    CGPDFScannerRelease(scanner)
    CGPDFContentStreamRelease(stream)
    CGPDFOperatorTableRelease(table)
    return s.origins
}

func number(_ value: CGFloat) -> String {
    var text = String(format: "%.2f", Double(value))
    while text.hasSuffix("0") { text.removeLast() }
    if text.hasSuffix(".") { text.removeLast() }
    return text == "-0" ? "0" : text
}

func quoted(_ text: String) -> String {
    var out = "\""
    for scalar in text.unicodeScalars {
        switch scalar {
        case "\"": out += "\\\""
        case "\\": out += "\\\\"
        case "\n": out += "\\n"
        case "\t": out += "\\t"
        case _ where scalar.value < 0x20: out += String(format: "\\u%04x", scalar.value)
        default: out.unicodeScalars.append(scalar)
        }
    }
    return out + "\""
}

struct Line {
    var text: String
    var bounds: CGRect
}

func lines(_ page: PDFPage) -> [Line] {
    let all = page.selection(for: NSRange(location: 0, length: page.numberOfCharacters))
    return (all?.selectionsByLine() ?? []).compactMap { line in
        let text = (line.string ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        return text.isEmpty ? nil : Line(text: text, bounds: line.bounds(for: page))
    }
}

let args = CommandLine.arguments
guard args.count == 2, let document = PDFDocument(url: URL(fileURLWithPath: args[1])) else {
    FileHandle.standardError.write("usage: pdflines FILE.pdf\n".data(using: .utf8)!)
    exit(2)
}

var output = "{\n  \"schema\": \"docrs_pdf_lines\",\n  \"version\": 1,\n  \"pages\": ["
for index in 0..<document.pageCount {
    let page = document.page(at: index)!
    let box = page.bounds(for: .mediaBox)
    let origins = page.pageRef.map(textOrigins) ?? []
    output += index == 0 ? "\n" : ",\n"
    output += "    {\"page\": \(index + 1), \"width\": \(number(box.width)), \"height\": \(number(box.height)), \"lines\": ["
    for (n, line) in lines(page).enumerated() {
        let b = line.bounds
        let slack = b.height / 2 + 0.5
        var counts: [CGFloat: Int] = [:]
        for origin in origins
        where origin.x >= b.minX - 2 * slack && origin.x <= b.maxX
            && origin.y >= b.minY - slack && origin.y <= b.maxY {
            counts[(origin.y * 100).rounded() / 100, default: 0] += 1
        }
        let baseline = counts.max { x, y in x.value != y.value ? x.value < y.value : x.key < y.key }?.key
        output += n == 0 ? "\n" : ",\n"
        output += "      {\"page\": \(index + 1), \"text\": \(quoted(line.text)), "
        output += "\"x\": \(number(b.minX - box.minX)), \"top\": \(number(box.maxY - b.maxY)), "
        output += "\"width\": \(number(b.width)), \"height\": \(number(b.height)), "
        output += "\"baseline\": \(baseline.map { number(box.maxY - $0) } ?? "null")}"
    }
    output += "\n    ]}"
}
output += "\n  ]\n}\n"
FileHandle.standardOutput.write(output.data(using: .utf8)!)
