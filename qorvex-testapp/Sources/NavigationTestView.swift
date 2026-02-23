import SwiftUI

struct NavigationTestView: View {
    @State private var showSheet = false
    @State private var showAlert = false
    @State private var showConfirmation = false
    @State private var statusText = "No action yet"

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 24) {

                    // MARK: - Push Navigation
                    Group {
                        Text("Push Navigation")
                            .font(.headline)
                        NavigationLink("Go to Detail", destination: DetailSubView())
                            .accessibilityIdentifier("nav-push-button")
                    }

                    Divider()

                    // MARK: - Sheet Presentation
                    Group {
                        Text("Sheet")
                            .font(.headline)
                        Button("Show Sheet") {
                            showSheet = true
                        }
                        .accessibilityIdentifier("nav-sheet-button")
                    }

                    Divider()

                    // MARK: - Alert Dialog
                    Group {
                        Text("Alert")
                            .font(.headline)
                        Button("Show Alert") {
                            showAlert = true
                        }
                        .accessibilityIdentifier("nav-alert-button")
                    }

                    Divider()

                    // MARK: - Confirmation Dialog
                    Group {
                        Text("Confirmation")
                            .font(.headline)
                        Button("Show Confirmation") {
                            showConfirmation = true
                        }
                        .accessibilityIdentifier("nav-confirm-button")
                    }

                    Divider()

                    // MARK: - Status
                    Text(statusText)
                        .accessibilityIdentifier("nav-status-label")
                }
                .padding()
            }
            .navigationTitle("Navigation")
            .sheet(isPresented: $showSheet, onDismiss: {
                statusText = "Sheet dismissed"
            }) {
                SheetSubView()
            }
            .alert("Test Alert", isPresented: $showAlert) {
                Button("OK") {
                    statusText = "Alert confirmed"
                }
                .accessibilityIdentifier("nav-alert-ok")
            } message: {
                Text("This is a test alert")
            }
            .confirmationDialog("Confirm Action", isPresented: $showConfirmation) {
                Button("Delete", role: .destructive) {
                    statusText = "Deleted"
                }
                .accessibilityIdentifier("nav-confirm-delete")
                Button("Cancel", role: .cancel) {
                    statusText = "Cancelled"
                }
            }
        }
    }
}

// MARK: - Detail Subview

private struct DetailSubView: View {
    var body: some View {
        VStack(spacing: 20) {
            Text("Detail Page")
                .accessibilityIdentifier("nav-detail-label")
        }
        .navigationTitle("Detail")
    }
}

// MARK: - Sheet Subview

private struct SheetSubView: View {
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(spacing: 20) {
            Text("Sheet Content")
                .accessibilityIdentifier("nav-sheet-content")
            Button("Dismiss") {
                dismiss()
            }
            .accessibilityIdentifier("nav-sheet-dismiss")
        }
    }
}
