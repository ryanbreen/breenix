#!/usr/bin/env swift
//
// generate-icon.swift -- draws the Breenix Run Inspector app icon and writes
// a complete .iconset directory (iconutil's expected 10 filenames) at the
// path given as the sole argument.
//
// Pure CoreGraphics vector drawing: no downloaded artwork, no system fonts,
// no external libraries. Every color and control point below is a literal
// constant, so re-running this script against the same Swift toolchain
// reproduces byte-identical PNGs (see Makefile's `icon` target and
// DESIGN.md for the iconutil packaging step and its own reproducibility
// note).
//
// Glyph: a bright "pulse" trace (the same idea as the app's own
// waveform.path.ecg tab icon for the Traces pane) crossing a dark rounded
// square, with a trailing dot -- read as "a run being watched", which is
// what this app does with gate boot output.
//
// Usage: swift Icon/generate-icon.swift <output-iconset-dir>

import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

// MARK: - Palette

// Background: a dark slate-navy vertical gradient (kernel/terminal feel).
private let backgroundTop = CGColor(srgbRed: 0x13 / 255.0, green: 0x1B / 255.0, blue: 0x29 / 255.0, alpha: 1)
private let backgroundBottom = CGColor(srgbRed: 0x0A / 255.0, green: 0x0F / 255.0, blue: 0x18 / 255.0, alpha: 1)

// Glyph: the same green the app uses for a passing verdict (SidebarView's
// `VerdictDisplayState.success` case resolves to SwiftUI's `Color.green`,
// whose macOS dark-appearance sRGB value is this literal).
private let pulseColor = CGColor(srgbRed: 0x30 / 255.0, green: 0xD1 / 255.0, blue: 0x58 / 255.0, alpha: 1)

// MARK: - Geometry (unit square, origin bottom-left, y up)

// A classic "QRS complex" pulse trace: flat, dip, spike, dip, flat, flat tail.
private let pulsePoints: [(x: CGFloat, y: CGFloat)] = [
    (0.14, 0.50),
    (0.32, 0.50),
    (0.40, 0.34),
    (0.47, 0.75),
    (0.54, 0.30),
    (0.61, 0.50),
    (0.86, 0.50),
]

private let dotCenter: (x: CGFloat, y: CGFloat) = (0.86, 0.50)

// Apple Big Sur-style "squircle" corner radius approximation, as a fraction
// of the icon's edge length.
private let cornerRadiusFraction: CGFloat = 0.2237

// Stroke width and dot radius, as fractions of the icon's edge length. Kept
// generous (>5% of the edge) so the glyph stays legible once downsampled to
// the 16x16 and 32x32 members of the iconset.
private let strokeWidthFraction: CGFloat = 0.052
private let dotRadiusFraction: CGFloat = 0.052

// MARK: - Drawing

private func drawIcon(size: Int) -> CGImage {
    let colorSpace = CGColorSpaceCreateDeviceRGB()
    guard let ctx = CGContext(
        data: nil,
        width: size,
        height: size,
        bitsPerComponent: 8,
        bytesPerRow: 0,
        space: colorSpace,
        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
    ) else {
        fatalError("failed to create a \(size)x\(size) bitmap context")
    }

    let s = CGFloat(size)

    // Background: rounded square, filled with a top-to-bottom gradient,
    // clipped to the rounded-rect path so corners are transparent.
    ctx.saveGState()
    let cornerRadius = s * cornerRadiusFraction
    let backgroundPath = CGPath(
        roundedRect: CGRect(x: 0, y: 0, width: s, height: s),
        cornerWidth: cornerRadius,
        cornerHeight: cornerRadius,
        transform: nil
    )
    ctx.addPath(backgroundPath)
    ctx.clip()
    guard let gradient = CGGradient(
        colorsSpace: colorSpace,
        colors: [backgroundTop, backgroundBottom] as CFArray,
        locations: [0, 1]
    ) else {
        fatalError("failed to build the background gradient")
    }
    ctx.drawLinearGradient(
        gradient,
        start: CGPoint(x: 0, y: s),
        end: CGPoint(x: 0, y: 0),
        options: []
    )
    ctx.restoreGState()

    // Foreground: the pulse trace and its trailing dot.
    ctx.saveGState()
    ctx.setStrokeColor(pulseColor)
    ctx.setLineWidth(s * strokeWidthFraction)
    ctx.setLineCap(.round)
    ctx.setLineJoin(.round)
    let scaledPoints = pulsePoints.map { CGPoint(x: $0.x * s, y: $0.y * s) }
    ctx.addLines(between: scaledPoints)
    ctx.strokePath()

    ctx.setFillColor(pulseColor)
    let dotRadius = s * dotRadiusFraction
    let dotRect = CGRect(
        x: dotCenter.x * s - dotRadius,
        y: dotCenter.y * s - dotRadius,
        width: dotRadius * 2,
        height: dotRadius * 2
    )
    ctx.fillEllipse(in: dotRect)
    ctx.restoreGState()

    guard let image = ctx.makeImage() else {
        fatalError("failed to render the \(size)x\(size) image")
    }
    return image
}

private func writePNG(_ image: CGImage, to url: URL) {
    guard let destination = CGImageDestinationCreateWithURL(
        url as CFURL,
        UTType.png.identifier as CFString,
        1,
        nil
    ) else {
        fatalError("failed to create a PNG destination at \(url.path)")
    }
    CGImageDestinationAddImage(destination, image, nil)
    guard CGImageDestinationFinalize(destination) else {
        fatalError("failed to write the PNG at \(url.path)")
    }
}

// MARK: - iconutil's expected .iconset member names

private let iconsetEntries: [(name: String, size: Int)] = [
    ("icon_16x16.png", 16),
    ("icon_16x16@2x.png", 32),
    ("icon_32x32.png", 32),
    ("icon_32x32@2x.png", 64),
    ("icon_128x128.png", 128),
    ("icon_128x128@2x.png", 256),
    ("icon_256x256.png", 256),
    ("icon_256x256@2x.png", 512),
    ("icon_512x512.png", 512),
    ("icon_512x512@2x.png", 1024),
]

// MARK: - Entry point

let arguments = CommandLine.arguments
guard arguments.count == 2 else {
    FileHandle.standardError.write(Data("usage: generate-icon.swift <output.iconset>\n".utf8))
    exit(1)
}

let outputDir = URL(fileURLWithPath: arguments[1], isDirectory: true)
do {
    try FileManager.default.createDirectory(at: outputDir, withIntermediateDirectories: true)
} catch {
    FileHandle.standardError.write(Data("failed to create \(outputDir.path): \(error)\n".utf8))
    exit(1)
}

for entry in iconsetEntries {
    let image = drawIcon(size: entry.size)
    writePNG(image, to: outputDir.appendingPathComponent(entry.name))
}

print("wrote \(iconsetEntries.count) PNGs to \(outputDir.path)")
