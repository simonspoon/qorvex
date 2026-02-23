# Makefile for building and running the qorvex test app.
#
# Prerequisites:
#   1. Xcode and iOS Simulator runtime installed
#   2. xcodegen (install via: brew install xcodegen)

BUILD_DIR = .build
PROJECT = QorvexTestApp.xcodeproj
SCHEME = QorvexTestApp

# Auto-detect the first booted iPhone simulator, falling back to any available iPhone.
BOOTED_SIM := $(shell xcrun simctl list devices booted -j 2>/dev/null | python3 -c \
	"import sys,json; d=json.load(sys.stdin)['devices']; print(next((dev['udid'] for devs in d.values() for dev in devs if dev.get('state')=='Booted' and 'iPhone' in dev.get('name','')), ''))" 2>/dev/null)
ANY_SIM := $(shell xcrun simctl list devices available -j 2>/dev/null | python3 -c \
	"import sys,json; d=json.load(sys.stdin)['devices']; print(next((dev['udid'] for rt in sorted(d.keys(),reverse=True) for dev in d[rt] if dev.get('isAvailable',False) and 'iPhone' in dev.get('name','')), ''))" 2>/dev/null)
SIM_UDID := $(or $(BOOTED_SIM),$(ANY_SIM))

DESTINATION ?= platform=iOS Simulator,id=$(SIM_UDID)

.PHONY: generate build install run clean

# Generate the Xcode project from project.yml
generate: project.yml
	@command -v xcodegen >/dev/null 2>&1 || { \
		echo "Error: xcodegen is not installed."; \
		echo "Install it with: brew install xcodegen"; \
		exit 1; \
	}
	xcodegen generate

# Build the app for Simulator
build: generate
	xcodebuild build \
		-project $(PROJECT) \
		-scheme $(SCHEME) \
		-destination '$(DESTINATION)' \
		-derivedDataPath $(BUILD_DIR)

# Install on booted Simulator
install: build
	@APP=$$(find $(BUILD_DIR) -name '$(SCHEME).app' -path '*/Debug-iphonesimulator/*' | head -1) && \
	xcrun simctl install booted "$$APP" && \
	echo "Installed $$APP"

# Launch on booted Simulator
run: install
	xcrun simctl launch booted com.qorvex.testapp

clean:
	rm -rf $(BUILD_DIR) $(PROJECT)
