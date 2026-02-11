# Makefile for building and running the qorvex agent via xcodebuild.
#
# Prerequisites:
#   1. Create an Xcode project (see README.md for setup instructions)
#   2. Xcode and iOS Simulator runtime installed

BUILD_DIR = .build
PROJECT = QorvexAgent.xcodeproj
SCHEME = QorvexAgentUITests
DESTINATION = platform=iOS Simulator,name=iPhone 16 Pro
TEST_CLASS = QorvexAgentUITests/QorvexAgentTests/testRunAgent

.PHONY: build test run clean

# Build the UI test bundle for testing
build:
	xcodebuild build-for-testing \
		-project $(PROJECT) \
		-scheme $(SCHEME) \
		-destination '$(DESTINATION)' \
		-derivedDataPath $(BUILD_DIR)

# Run the agent (starts the TCP server on the simulator)
run: build
	xcodebuild test-without-building \
		-project $(PROJECT) \
		-scheme $(SCHEME) \
		-destination '$(DESTINATION)' \
		-derivedDataPath $(BUILD_DIR) \
		-only-testing:$(TEST_CLASS)

# Direct test (build + run in one step)
test:
	xcodebuild test \
		-project $(PROJECT) \
		-scheme $(SCHEME) \
		-destination '$(DESTINATION)' \
		-derivedDataPath $(BUILD_DIR) \
		-only-testing:$(TEST_CLASS)

clean:
	rm -rf $(BUILD_DIR)
	xcodebuild clean -project $(PROJECT) -scheme $(SCHEME) 2>/dev/null || true
