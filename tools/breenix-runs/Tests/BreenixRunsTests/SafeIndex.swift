/// An index past the end is nil, so a short argument list fails the one assertion instead of trapping the test process.
extension Array {
    subscript(safe index: Int) -> Element? { indices.contains(index) ? self[index] : nil }
}
