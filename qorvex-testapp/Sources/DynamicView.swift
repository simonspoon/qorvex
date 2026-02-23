import SwiftUI

struct DynamicView: View {
    // MARK: - State

    @State private var showDelayed = false
    @State private var showBrief = false
    @State private var isLoading = false
    @State private var loadingDone = false
    @State private var isVisible = false
    @State private var counterRunning = false
    @State private var counterValue = 0
    @State private var counterTimer: Timer.TimerPublisher?
    @State private var counterCancellable: Any? = nil

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 24) {

                // MARK: - Delayed Appearance

                Group {
                    Text("Delayed Appearance")
                        .font(.headline)
                    Button("Show After Delay") {
                        DispatchQueue.main.asyncAfter(deadline: .now() + 2) {
                            showDelayed = true
                        }
                    }
                    .accessibilityIdentifier("dynamic-show-delayed")

                    if showDelayed {
                        Text("I appeared!")
                            .accessibilityIdentifier("dynamic-delayed-label")
                            .transition(.opacity)
                    }
                }
                .animation(.default, value: showDelayed)

                Divider()

                // MARK: - Auto-Disappearing Element

                Group {
                    Text("Auto-Disappearing Element")
                        .font(.headline)
                    Button("Show Briefly") {
                        showBrief = true
                        DispatchQueue.main.asyncAfter(deadline: .now() + 3) {
                            showBrief = false
                        }
                    }
                    .accessibilityIdentifier("dynamic-show-brief")

                    if showBrief {
                        Text("Now you see me")
                            .accessibilityIdentifier("dynamic-brief-label")
                            .transition(.opacity)
                    }
                }
                .animation(.default, value: showBrief)

                Divider()

                // MARK: - Loading Indicator

                Group {
                    Text("Loading Indicator")
                        .font(.headline)
                    Button("Start Loading") {
                        isLoading = true
                        loadingDone = false
                        DispatchQueue.main.asyncAfter(deadline: .now() + 2) {
                            isLoading = false
                            loadingDone = true
                        }
                    }
                    .accessibilityIdentifier("dynamic-start-loading")

                    if isLoading {
                        ProgressView()
                            .accessibilityIdentifier("dynamic-loading-spinner")
                            .transition(.opacity)
                    }

                    if loadingDone {
                        Text("Loading complete")
                            .accessibilityIdentifier("dynamic-loading-done")
                            .transition(.opacity)
                    }
                }
                .animation(.default, value: isLoading)
                .animation(.default, value: loadingDone)

                Divider()

                // MARK: - Toggle Visibility

                Group {
                    Text("Toggle Visibility")
                        .font(.headline)
                    Button("Toggle Element") {
                        isVisible.toggle()
                    }
                    .accessibilityIdentifier("dynamic-toggle-visibility")

                    if isVisible {
                        Text("Visible Element")
                            .accessibilityIdentifier("dynamic-togglable")
                            .transition(.opacity)
                    }
                }
                .animation(.default, value: isVisible)

                Divider()

                // MARK: - Counter with Auto-Increment

                Group {
                    Text("Counter with Auto-Increment")
                        .font(.headline)
                    Button("Start Counter") {
                        guard !counterRunning else { return }
                        counterRunning = true
                        counterValue = 0
                    }
                    .accessibilityIdentifier("dynamic-start-counter")

                    if counterRunning {
                        Text("Count: \(counterValue)")
                            .accessibilityIdentifier("dynamic-counter-value")

                        Button("Stop Counter") {
                            counterRunning = false
                        }
                        .accessibilityIdentifier("dynamic-stop-counter")
                    }
                }
                .animation(.default, value: counterRunning)
                .onReceive(
                    Timer.publish(every: 1, on: .main, in: .common).autoconnect()
                ) { _ in
                    if counterRunning {
                        counterValue += 1
                    }
                }

                Divider()

                // MARK: - Reset

                Group {
                    Text("Reset")
                        .font(.headline)
                    Button("Reset All") {
                        showDelayed = false
                        showBrief = false
                        isLoading = false
                        loadingDone = false
                        isVisible = false
                        counterRunning = false
                        counterValue = 0
                    }
                    .accessibilityIdentifier("dynamic-reset")
                }

            }
            .padding()
        }
        .navigationTitle("Dynamic")
    }
}
