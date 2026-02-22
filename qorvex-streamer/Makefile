.PHONY: build clean install

build:
	swift build -c release

clean:
	swift package clean

install: build
	cp .build/release/qorvex-streamer /usr/local/bin/
