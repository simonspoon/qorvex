# Makefile for building and running the qorvex test app.
#
# Prerequisites:
#   1. Xcode and iOS Simulator runtime installed
#   2. xcodegen (install via: brew install xcodegen)

BUILD_DIR = .build
PROJECT = QorvexTestApp.xcodeproj
SCHEME = QorvexTestApp

# Simulator UDID to target. Required (no auto-detection, so a shared machine
# never builds for or installs onto another session's device), e.g.:
#   make build QORVEX_TEST_SIM=<udid>
# DESTINATION may be set explicitly instead for build; install and run always
# need QORVEX_TEST_SIM because simctl takes a UDID.
ifneq ($(QORVEX_TEST_SIM),)
DESTINATION ?= platform=iOS Simulator,id=$(QORVEX_TEST_SIM)
endif

.PHONY: generate build install run clean require-destination require-sim

# Generate the Xcode project from project.yml
generate: project.yml
	@command -v xcodegen >/dev/null 2>&1 || { \
		echo "Error: xcodegen is not installed."; \
		echo "Install it with: brew install xcodegen"; \
		exit 1; \
	}
	xcodegen generate

# Guards: fail only the targets that need a simulator.
require-destination:
	@if [ -z "$(DESTINATION)" ]; then \
		echo "Error: QORVEX_TEST_SIM is not set. Set it to a simulator UDID (or set DESTINATION)."; \
		exit 1; \
	fi

require-sim:
	@if [ -z "$(QORVEX_TEST_SIM)" ]; then \
		echo "Error: QORVEX_TEST_SIM is not set. Set it to a simulator UDID."; \
		exit 1; \
	fi

# Build the app for Simulator
build: require-destination generate
	xcodebuild build \
		-project $(PROJECT) \
		-scheme $(SCHEME) \
		-destination '$(DESTINATION)' \
		-derivedDataPath $(BUILD_DIR)

# Install on the Simulator named by QORVEX_TEST_SIM
install: require-sim build
	@APP=$$(find $(BUILD_DIR) -name '$(SCHEME).app' -path '*/Debug-iphonesimulator/*' | head -1) && \
	xcrun simctl install $(QORVEX_TEST_SIM) "$$APP" && \
	echo "Installed $$APP"

# Launch on the Simulator named by QORVEX_TEST_SIM
run: install
	xcrun simctl launch $(QORVEX_TEST_SIM) com.qorvex.testapp

clean:
	rm -rf $(BUILD_DIR) $(PROJECT)
