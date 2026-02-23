import SwiftUI

struct ControlsView: View {
    @State private var tapCount = 0
    @State private var wifiEnabled = false
    @State private var volume: Double = 50
    @State private var quantity = 1
    @State private var selectedSize = "Medium"
    @State private var deleted = false

    private let sizes = ["Small", "Medium", "Large"]

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 24) {

                // MARK: - Tap Button
                Group {
                    Text("Tap Button")
                        .font(.headline)
                    Button("Tap Me") {
                        tapCount += 1
                    }
                    .accessibilityIdentifier("controls-tap-button")
                    Text("Tapped: \(tapCount)")
                        .accessibilityIdentifier("controls-tap-count")
                }

                Divider()

                // MARK: - Toggle
                Group {
                    Text("Toggle")
                        .font(.headline)
                    Toggle("Wi-Fi", isOn: $wifiEnabled)
                        .accessibilityIdentifier("controls-toggle-wifi")
                    Text(wifiEnabled ? "On" : "Off")
                        .accessibilityIdentifier("controls-wifi-status")
                }

                Divider()

                // MARK: - Slider
                Group {
                    Text("Slider")
                        .font(.headline)
                    Slider(value: $volume, in: 0...100)
                        .accessibilityIdentifier("controls-slider-volume")
                        .accessibilityLabel("Volume")
                    Text("Volume: \(Int(volume))")
                        .accessibilityIdentifier("controls-slider-value")
                }

                Divider()

                // MARK: - Stepper
                Group {
                    Text("Stepper")
                        .font(.headline)
                    Stepper("Quantity: \(quantity)", value: $quantity, in: 0...20)
                        .accessibilityIdentifier("controls-stepper-quantity")
                    Text("Quantity: \(quantity)")
                        .accessibilityIdentifier("controls-stepper-value")
                }

                Divider()

                // MARK: - Segmented Picker
                Group {
                    Text("Segmented Picker")
                        .font(.headline)
                    Picker("Size", selection: $selectedSize) {
                        ForEach(sizes, id: \.self) { size in
                            Text(size).tag(size)
                        }
                    }
                    .pickerStyle(.segmented)
                    .accessibilityIdentifier("controls-picker-size")
                    .accessibilityLabel("Size")
                    Text(selectedSize)
                        .accessibilityIdentifier("controls-picker-value")
                }

                Divider()

                // MARK: - Destructive Button
                Group {
                    Text("Destructive Button")
                        .font(.headline)
                    Button("Delete", role: .destructive) {
                        deleted = true
                    }
                    .accessibilityIdentifier("controls-delete-button")
                    if deleted {
                        Text("Deleted!")
                            .accessibilityIdentifier("controls-delete-status")
                    }
                }
            }
            .padding()
        }
        .navigationTitle("Controls")
    }
}
