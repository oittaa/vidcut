# The common tasks on Linux and macOS: `make help` lists them. Each one is a
# short command, mostly a script in scripts/, which also runs on Windows from
# Git Bash. Plain GNU make 3.81 (macOS's) runs this file.

TARGET_DIR := $(or $(CARGO_TARGET_DIR),target)
VIDCUT := $(TARGET_DIR)/release/vidcut
PREFIX ?= $(HOME)/.local

.DEFAULT_GOAL := build
.PHONY: help build run check check-full fmt test hooks doctor media install clean

help: ## List the targets
	@awk 'BEGIN { FS = ":.*## " } /^[a-z-]+:.*## / { printf "  make %-11s %s\n", $$1, $$2 }' $(MAKEFILE_LIST)

build: ## Build vidcut, optimized (the default)
	cargo build --release -p vidcut --bin vidcut

run: build ## Start the GUI. make run P=talk.vidcut opens a project
	$(VIDCUT) $(if $(P),-p "$(P)")

check: ## The gate before every commit: binary files, fmt, clippy, unit tests
	scripts/check.sh

check-full: ## The gate after bigger changes: check, plus every integration test
	scripts/check.sh full

fmt: ## Format the code
	cargo fmt --all

test: ## The unit tests alone
	cargo test --workspace --lib --bins

hooks: ## Run check before every commit (once per clone)
	git config core.hooksPath .githooks

doctor: ## Check Rust, pkg-config and FFmpeg, then show the GPUs and encoders
	@command -v cargo >/dev/null || { echo "cargo is missing: install rustup (README.md, Requirements)"; exit 1; }
	@command -v pkg-config >/dev/null || { echo "pkg-config is missing (README.md, Requirements)"; exit 1; }
	@pkg-config --atleast-version=62 libavcodec || { echo "pkg-config finds no FFmpeg 8.0 or newer (libavcodec 62): $$(pkg-config --modversion libavcodec 2>/dev/null || echo none). See README.md, Requirements"; exit 1; }
	@echo "$$(cargo --version), FFmpeg's libavcodec $$(pkg-config --modversion libavcodec)"
	@if [ -x "$(VIDCUT)" ]; then "$(VIDCUT)" doctor; else echo "make build, then make doctor shows the GPUs, encoders and audio devices"; fi

media: ## Download the test clips into $VIDCUT_TEST_DATA (~/Downloads/vidcut-tmp)
	scripts/fetch-test-media.sh

install: build ## Copy vidcut to ~/.local/bin, or to PREFIX/bin
	mkdir -p "$(PREFIX)/bin"
	cp "$(VIDCUT)" "$(PREFIX)/bin/vidcut"
	@case ":$$PATH:" in *":$(PREFIX)/bin:"*) ;; *) echo "$(PREFIX)/bin isn't on PATH: add it to run vidcut by name";; esac

clean: ## Remove the build
	cargo clean
