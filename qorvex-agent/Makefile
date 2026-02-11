# Makefile for building and running the qorvex agent via xcodebuild.
#
# Prerequisites:
#   1. Xcode and iOS Simulator runtime installed
#   2. xcodegen (install via: make install-xcodegen)
#
# Override the simulator destination:
#   make build DESTINATION='platform=iOS Simulator,name=iPhone 15 Pro'

BUILD_DIR = .build
PROJECT = QorvexAgent.xcodeproj
SCHEME = QorvexAgentUITests
TEST_CLASS = QorvexAgentUITests/QorvexAgentTests/testRunAgent

# Auto-detect the first booted iPhone simulator, falling back to any available iPhone.
BOOTED_SIM := $(shell xcrun simctl list devices booted -j 2>/dev/null | python3 -c \
	"import sys,json; d=json.load(sys.stdin)['devices']; print(next((dev['udid'] for devs in d.values() for dev in devs if dev.get('state')=='Booted' and 'iPhone' in dev.get('name','')), ''))" 2>/dev/null)
ANY_SIM := $(shell xcrun simctl list devices available -j 2>/dev/null | python3 -c \
	"import sys,json; d=json.load(sys.stdin)['devices']; print(next((dev['udid'] for rt in sorted(d.keys(),reverse=True) for dev in d[rt] if dev.get('isAvailable',False) and 'iPhone' in dev.get('name','')), ''))" 2>/dev/null)
SIM_UDID := $(or $(BOOTED_SIM),$(ANY_SIM))

DESTINATION ?= platform=iOS Simulator,id=$(SIM_UDID)

.PHONY: generate build test run clean install-xcodegen

# Generate the Xcode project from project.yml
generate: project.yml
	@command -v xcodegen >/dev/null 2>&1 || { \
		echo "Error: xcodegen is not installed."; \
		echo "Install it with: brew install xcodegen"; \
		exit 1; \
	}
	xcodegen generate

# Build the UI test bundle for testing
build: generate
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
test: generate
	xcodebuild test \
		-project $(PROJECT) \
		-scheme $(SCHEME) \
		-destination '$(DESTINATION)' \
		-derivedDataPath $(BUILD_DIR) \
		-only-testing:$(TEST_CLASS)

clean:
	rm -rf $(BUILD_DIR) $(PROJECT)

install-xcodegen:
	brew install xcodegen
