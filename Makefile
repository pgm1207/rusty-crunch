NAME := rusty-crunch
VERSION := $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
PREFIX ?= /usr/local
BINDIR ?= $(PREFIX)/bin
MANDIR ?= $(PREFIX)/share/man/man1
DESTDIR ?=
CARGO ?= cargo
TARGET := target/release/$(NAME)

.PHONY: all help build release fmt fmt-check lint test check install uninstall man package clean

all: help

help:
	@echo "$(NAME) $(VERSION)"
	@echo ""
	@echo "Usage: make <target>"
	@echo ""
	@echo "  build       Debug build"
	@echo "  release     Optimized release build"
	@echo "  fmt         Format the source"
	@echo "  fmt-check   Verify formatting (CI)"
	@echo "  lint        Run clippy with -D warnings (CI)"
	@echo "  test        Run the unit/integration tests"
	@echo "  check       fmt-check + lint + test + release build"
	@echo "  install     Install binary + man page to PREFIX ($(PREFIX))"
	@echo "  uninstall   Remove the installed binary + man page"
	@echo "  package     Build a reproducible tarball in dist/"
	@echo "  clean       Remove build artifacts"

build:
	$(CARGO) build

release:
	$(CARGO) build --release

fmt:
	$(CARGO) fmt --all

fmt-check:
	$(CARGO) fmt --all -- --check

lint:
	$(CARGO) clippy --all-targets -- -D warnings

test:
	$(CARGO) test

check: fmt-check lint test release

install: release
	install -d "$(DESTDIR)$(BINDIR)"
	install -m 0755 "$(TARGET)" "$(DESTDIR)$(BINDIR)/$(NAME)"
	@if [ -f "$(NAME).1" ]; then \
		install -d "$(DESTDIR)$(MANDIR)"; \
		install -m 0644 "$(NAME).1" "$(DESTDIR)$(MANDIR)/$(NAME).1"; \
		echo "installed $(NAME).1"; \
	fi

uninstall:
	rm -f "$(DESTDIR)$(BINDIR)/$(NAME)"
	rm -f "$(DESTDIR)$(MANDIR)/$(NAME).1"

man: $(NAME).1

$(NAME).1: $(NAME).1

package: release
	mkdir -p dist
	tar \
		--sort=name \
		--mtime='@0' \
		--owner=0 --group=0 --numeric-owner \
		-C target/release \
		-czf "dist/$(NAME)-$(VERSION)-linux-x86_64.tar.gz" "$(NAME)"
	@echo "Wrote dist/$(NAME)-$(VERSION)-linux-x86_64.tar.gz"
	@sha256sum "dist/$(NAME)-$(VERSION)-linux-x86_64.tar.gz" || true

clean:
	$(CARGO) clean
	rm -rf dist
