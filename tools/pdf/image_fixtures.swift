// Writes the image fixtures of tests/fixtures/pdf/images that come from
// macOS ImageIO, plus photo.ppm and photo.pgm for libjpeg's cjpeg
// (tests/fixtures/pdf/images/PROVENANCE.md).
//
//   swift tools/pdf/image_fixtures.swift OUT_DIR

import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

let out = URL(fileURLWithPath: CommandLine.arguments[1])
let (width, height) = (48, 32)

/// A colour gradient with a dark disc and a white bar.
func photo(_ space: CGColorSpace) -> CGImage {
    let context = CGContext(
        data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
        space: CGColorSpace(name: CGColorSpace.sRGB)!,
        bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue)!
    for y in 0..<height {
        for x in 0..<width {
            context.setFillColor(
                red: CGFloat(x) / CGFloat(width - 1), green: CGFloat(y) / CGFloat(height - 1),
                blue: 0.6, alpha: 1)
            context.fill(CGRect(x: x, y: y, width: 1, height: 1))
        }
    }
    context.setFillColor(red: 0.1, green: 0.1, blue: 0.2, alpha: 1)
    context.fillEllipse(in: CGRect(x: 6, y: 6, width: 20, height: 20))
    context.setFillColor(red: 1, green: 1, blue: 1, alpha: 1)
    context.fill(CGRect(x: 30, y: 4, width: 12, height: 24))
    return context.makeImage()!.copy(colorSpace: space) ?? convert(context.makeImage()!, to: space)
}

func convert(_ image: CGImage, to space: CGColorSpace) -> CGImage {
    let alpha = space.model == .cmyk ? CGImageAlphaInfo.none : .noneSkipLast
    let context = CGContext(
        data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
        space: space, bitmapInfo: alpha.rawValue)!
    context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
    return context.makeImage()!
}

/// A blue rounded square with a hole, on a transparent background.
func logo() -> CGImage {
    let context = CGContext(
        data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
        space: CGColorSpace(name: CGColorSpace.sRGB)!,
        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    context.setFillColor(red: 0.1, green: 0.3, blue: 0.8, alpha: 1)
    context.addPath(
        CGPath(
            roundedRect: CGRect(x: 4, y: 2, width: 40, height: 28), cornerWidth: 8,
            cornerHeight: 8, transform: nil))
    context.addEllipse(in: CGRect(x: 16, y: 8, width: 16, height: 16))
    context.fillPath(using: .evenOdd)
    context.setFillColor(red: 0.9, green: 0.5, blue: 0.1, alpha: 0.5)
    context.fill(CGRect(x: 0, y: 12, width: 48, height: 8))
    return context.makeImage()!
}

func write(_ name: String, _ type: UTType, _ frames: [CGImage], _ properties: [CFString: Any] = [:]) {
    let destination = CGImageDestinationCreateWithURL(
        out.appendingPathComponent(name) as CFURL, type.identifier as CFString, frames.count, nil)!
    CGImageDestinationSetProperties(destination, properties as CFDictionary)
    for frame in frames {
        CGImageDestinationAddImage(destination, frame, properties as CFDictionary)
    }
    precondition(CGImageDestinationFinalize(destination), name)
}

/// Binary PPM (or PGM) of the photo for cjpeg.
func netpbm(_ name: String, grey: Bool) {
    let image = convert(photo(CGColorSpace(name: CGColorSpace.sRGB)!), to: CGColorSpace(name: CGColorSpace.sRGB)!)
    let data = image.dataProvider!.data! as Data
    var body = Data("\(grey ? "P5" : "P6")\n\(width) \(height)\n255\n".utf8)
    for y in 0..<height {
        for x in 0..<width {
            let at = y * image.bytesPerRow + x * 4
            let (r, g, b) = (Double(data[at]), Double(data[at + 1]), Double(data[at + 2]))
            if grey {
                body.append(UInt8((0.299 * r + 0.587 * g + 0.114 * b).rounded()))
            } else {
                body.append(contentsOf: [data[at], data[at + 1], data[at + 2]])
            }
        }
    }
    try! body.write(to: out.appendingPathComponent(name))
}

let quality: [CFString: Any] = [kCGImageDestinationLossyCompressionQuality: 0.9]
write("photo-cmyk.jpg", .jpeg, [photo(CGColorSpace(name: CGColorSpace.genericCMYK)!)], quality)
write("photo-p3.jpg", .jpeg, [photo(CGColorSpace(name: CGColorSpace.displayP3)!)], quality)
write("logo.png", .png, [logo()])
let frames = (0..<3).map { shift -> CGImage in
    let context = CGContext(
        data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
        space: CGColorSpace(name: CGColorSpace.sRGB)!,
        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    context.setFillColor(red: 0.9, green: 0.2, blue: 0.2, alpha: 1)
    context.fill(CGRect(x: 4 + shift * 12, y: 8, width: 16, height: 16))
    context.setFillColor(red: 0.2, green: 0.6, blue: 0.2, alpha: 1)
    context.fill(CGRect(x: 0, y: 0, width: 48, height: 4))
    return context.makeImage()!
}
write(
    "animated.gif", .gif, frames,
    [kCGImagePropertyGIFDictionary: [kCGImagePropertyGIFDelayTime: 0.5, kCGImagePropertyGIFLoopCount: 0]])
netpbm("photo.ppm", grey: false)
netpbm("photo.pgm", grey: true)
