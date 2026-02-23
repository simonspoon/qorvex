import SwiftUI

@main
struct QorvexTestApp: App {
    var body: some Scene {
        WindowGroup {
            TabView {
                ControlsView()
                    .tabItem {
                        Label("Controls", systemImage: "slider.horizontal.3")
                    }

                TextInputView()
                    .tabItem {
                        Label("Text Input", systemImage: "keyboard")
                    }

                NavigationTestView()
                    .tabItem {
                        Label("Navigation", systemImage: "arrow.triangle.turn.up.right.diamond")
                    }

                ScrollGesturesView()
                    .tabItem {
                        Label("Gestures", systemImage: "hand.draw")
                    }

                DynamicView()
                    .tabItem {
                        Label("Dynamic", systemImage: "clock.arrow.circlepath")
                    }
            }
            .accessibilityIdentifier("main-tab-view")
        }
    }
}
