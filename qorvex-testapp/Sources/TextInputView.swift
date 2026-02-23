import SwiftUI

struct TextInputView: View {
    @State private var username = ""
    @State private var email = ""
    @State private var password = ""
    @State private var search = ""
    @State private var notes = ""
    @State private var submitResult: String?

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 24) {

                // MARK: - Username
                Group {
                    Text("Username")
                        .font(.headline)
                    TextField("Username", text: $username)
                        .textFieldStyle(.roundedBorder)
                        .accessibilityIdentifier("text-username-field")
                }

                Divider()

                // MARK: - Email
                Group {
                    Text("Email")
                        .font(.headline)
                    TextField("Email", text: $email)
                        .textFieldStyle(.roundedBorder)
                        .keyboardType(.emailAddress)
                        .accessibilityIdentifier("text-email-field")
                }

                Divider()

                // MARK: - Password
                Group {
                    Text("Password")
                        .font(.headline)
                    SecureField("Password", text: $password)
                        .textFieldStyle(.roundedBorder)
                        .accessibilityIdentifier("text-password-field")
                }

                Divider()

                // MARK: - Search
                Group {
                    Text("Search")
                        .font(.headline)
                    HStack {
                        Image(systemName: "magnifyingglass")
                            .foregroundColor(.secondary)
                        TextField("Search...", text: $search)
                            .accessibilityIdentifier("text-search-field")
                    }
                    .padding(8)
                    .background(Color(.systemGray6))
                    .cornerRadius(8)
                }

                Divider()

                // MARK: - Notes
                Group {
                    Text("Notes")
                        .font(.headline)
                    TextEditor(text: $notes)
                        .frame(minHeight: 100)
                        .overlay(
                            RoundedRectangle(cornerRadius: 8)
                                .stroke(Color.secondary, lineWidth: 1)
                        )
                        .accessibilityIdentifier("text-notes-editor")
                }

                Divider()

                // MARK: - Actions
                Group {
                    Text("Actions")
                        .font(.headline)
                    HStack(spacing: 16) {
                        Button("Submit") {
                            submitResult = "Submitted: \(username)"
                        }
                        .accessibilityIdentifier("text-submit-button")

                        Button("Clear All") {
                            username = ""
                            email = ""
                            password = ""
                            search = ""
                            notes = ""
                            submitResult = nil
                        }
                        .accessibilityIdentifier("text-clear-button")
                    }

                    if let result = submitResult {
                        Text(result)
                            .accessibilityIdentifier("text-submit-result")
                    }
                }

                Divider()

                // MARK: - Live Preview
                Group {
                    Text("Live Preview")
                        .font(.headline)
                    Text("Username: \(username)")
                        .accessibilityIdentifier("text-username-value")
                    Text("Email: \(email)")
                        .accessibilityIdentifier("text-email-value")
                }
            }
            .padding()
        }
        .navigationTitle("Text Input")
    }
}
