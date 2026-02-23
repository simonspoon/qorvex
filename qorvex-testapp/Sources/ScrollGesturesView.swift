import SwiftUI

struct ScrollGesturesView: View {
    @State private var longPressed = false
    @State private var swipeDirection = ""
    @State private var tapLocation: CGPoint? = nil

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 24) {

                // MARK: - Scrollable List
                Group {
                    Text("Scrollable List")
                        .font(.headline)
                    List {
                        ForEach(1...50, id: \.self) { n in
                            Text("Item \(n)")
                                .accessibilityIdentifier("scroll-item-\(n)")
                        }
                    }
                    .accessibilityIdentifier("scroll-list")
                    .frame(height: 300)
                    .listStyle(.plain)
                }

                Divider()

                // MARK: - Long-Press Target
                Group {
                    Text("Long-Press Target")
                        .font(.headline)
                    RoundedRectangle(cornerRadius: 12)
                        .fill(longPressed ? Color.green : Color.blue)
                        .frame(width: 100, height: 100)
                        .onLongPressGesture(minimumDuration: 1.0) {
                            longPressed = true
                        }
                        .accessibilityIdentifier("gesture-longpress-target")
                    Text(longPressed ? "Long pressed!" : "Tap and hold")
                        .accessibilityIdentifier("gesture-longpress-status")
                }

                Divider()

                // MARK: - Drag/Swipe Area
                Group {
                    Text("Drag / Swipe Area")
                        .font(.headline)
                    Rectangle()
                        .fill(Color.orange.opacity(0.3))
                        .frame(maxWidth: .infinity)
                        .frame(height: 150)
                        .gesture(
                            DragGesture(minimumDistance: 20)
                                .onEnded { value in
                                    let dx = value.translation.width
                                    let dy = value.translation.height
                                    if abs(dx) > abs(dy) {
                                        swipeDirection = dx > 0 ? "right" : "left"
                                    } else {
                                        swipeDirection = dy > 0 ? "down" : "up"
                                    }
                                }
                        )
                        .accessibilityIdentifier("gesture-swipe-area")
                    Text(swipeDirection.isEmpty ? "Swipe here" : "Swiped: \(swipeDirection)")
                        .accessibilityIdentifier("gesture-swipe-status")
                }

                Divider()

                // MARK: - Tap Coordinate Display
                Group {
                    Text("Tap Coordinate Display")
                        .font(.headline)
                    Rectangle()
                        .fill(Color.purple.opacity(0.2))
                        .frame(maxWidth: .infinity)
                        .frame(height: 150)
                        .gesture(
                            SpatialTapGesture()
                                .onEnded { value in
                                    tapLocation = value.location
                                }
                        )
                        .accessibilityIdentifier("gesture-tap-area")
                    Text(tapLocationText)
                        .accessibilityIdentifier("gesture-tap-location")
                }
            }
            .padding()
        }
        .navigationTitle("Scroll & Gestures")
    }

    private var tapLocationText: String {
        guard let pt = tapLocation else {
            return "Tap the area above"
        }
        return "Tapped at: \(Int(pt.x)), \(Int(pt.y))"
    }
}
