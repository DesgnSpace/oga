DIST := dist
APP := $(DIST)/Oga.app
VERSION := $(shell tr -d ' \n' < VERSION 2>/dev/null)
TARGET := universal-apple-darwin
# The newest v* tag names the build (v stripped); unreleased trees start at 0.0.1.
BINDIR ?= $(HOME)/.local/bin
OGA_DB ?= $(HOME)/.oga/oga.db
CUTOVER_BACKUP ?= $(OGA_DB).cutover-backup
CUTOVER_SKIP_BACKUP ?= 0
CUTOVER_RESTORE ?= 0
BUILD_STAMP ?= $(shell sha="$$(git rev-parse --short HEAD 2>/dev/null || echo nogit)"; printf '%s-%s' "$$sha" "$$(date +%Y%m%d%H%M%S)")
RUST_MANIFEST := rust/Cargo.toml
RUST_SERVER_BINARY := rust/target/release/oga-cli
RUST_BROKER := $(DIST)/oga-server
# Development brokers stay off port 7331 and the installed database, so a
# `make dev-broker` cannot write behind the broker the app is running.
DEV_PORT ?= 7399
DEV_DB ?= $(HOME)/.oga/dev.db
# A user-owned install directory stays out of macOS App Management, which
# refuses a terminal write into /Applications until that terminal is approved
# again after every update. INSTALL_DIR overrides it.
INSTALL_DIR ?= $(HOME)/Applications
# The development build installs under its own name and bundle identifier, so
# it sits beside a released Oga.app instead of replacing it: LaunchServices,
# the Dock and Spotlight see two apps. Both share port 7331 and ~/.oga, so only
# one runs at a time — quit one, open the other.
LOCAL_APP_NAME := Oga (local)
INSTALL_APP := $(INSTALL_DIR)/$(LOCAL_APP_NAME).app
# Where `cargo tauri build` leaves the macOS bundle it assembles.
TAURI_APP := rust/target/release/bundle/macos/Oga.app

# The codesigning identity. Picks up a Developer ID from the keychain when one
# is installed; otherwise signs ad-hoc, which needs no certificate and still
# produces an app macOS will run locally. A release overrides it explicitly.
DEVELOPER_ID_APP ?= $(shell security find-identity -v -p codesigning 2>/dev/null | awk -F'"' '/Developer ID Application/ {print $$2; exit}')
ifeq ($(strip $(DEVELOPER_ID_APP)),)
DEVELOPER_ID_APP := -
endif

BROKER_BUILD := server
BROKER_BINARY := $(RUST_BROKER)

.PHONY: dev dev-broker dev-desktop server rust-fmt rust-lint rust-test smoke desktop desktop-app bundle app-bundle install changelog deploy-landing clean sync-version publish release _publish major minor fix check-publish-tools

dev: dev-desktop

# The Rust broker, rebuilt on demand and kept off the installed port and
# database so the app's broker keeps serving while this one runs.
dev-broker:
	OGA_PORT=$(DEV_PORT) OGA_DB=$(DEV_DB) cargo run --manifest-path $(RUST_MANIFEST) --package oga-cli -- serve

# The Tauri shell against the React UI; Vite serves the dev bundle.
dev-desktop:
	@command -v bun >/dev/null 2>&1 || { echo "dev-desktop: bun is missing — install it from https://bun.sh"; exit 2; }
	@test -d web/node_modules || (cd web && bun install)
	cd rust/apps/oga-desktop && cargo tauri dev

rust-fmt:
	cargo fmt --manifest-path $(RUST_MANIFEST) --all --check

rust-lint:
	cargo clippy --manifest-path $(RUST_MANIFEST) --workspace --all-targets --all-features -- -D warnings

rust-test:
	cargo test --manifest-path $(RUST_MANIFEST) --workspace

# Starts the built broker on a scratch port and database, then retires it.
smoke: server
	bash rust/packaging/smoke-test.sh $(RUST_SERVER_BINARY)

desktop:
	bash rust/packaging/build.sh

desktop-app:
	bash rust/packaging/build.sh --app-only

server:
	mkdir -p $(DIST)
	OGA_BUILD_STAMP="$(BUILD_STAMP)" cargo build --manifest-path $(RUST_MANIFEST) --package oga-cli --release --locked
	install -m 755 $(RUST_SERVER_BINARY) $(RUST_BROKER)

# The Tauri build assembles the app — icon, Info.plist, and the broker
# sidecar — and this stages it under dist/ where install and notarize expect it.
bundle: $(BROKER_BUILD) desktop

app-bundle: $(BROKER_BUILD) desktop-app

bundle app-bundle:
	rm -rf $(APP)
	mkdir -p $(DIST)
	@test -d $(TAURI_APP) || { echo "Error: $(TAURI_APP) not found; run: make desktop-app"; exit 1; }
	ditto $(TAURI_APP) $(APP)
	install -m 755 $(BROKER_BINARY) $(APP)/Contents/Resources/oga-server
	@if [ "$(HARDENED)" = "1" ]; then \
		codesign --force --options runtime --timestamp --entitlements rust/apps/oga-desktop/entitlements.plist --sign "$(DEVELOPER_ID_APP)" "$(APP)/Contents/Resources/oga-server"; \
		codesign --force --options runtime --timestamp --entitlements rust/apps/oga-desktop/entitlements.plist --sign "$(DEVELOPER_ID_APP)" "$(APP)"; \
	else \
		codesign --force --entitlements rust/apps/oga-desktop/entitlements.plist --sign "$(DEVELOPER_ID_APP)" "$(APP)/Contents/Resources/oga-server"; \
		codesign --force --entitlements rust/apps/oga-desktop/entitlements.plist --sign "$(DEVELOPER_ID_APP)" "$(APP)"; \
	fi

install: app-bundle
	@# Retiring the broker stops whatever it is driving before replacement.
	@OGA_DB="$(OGA_DB)" sh scripts/install-preflight.sh $(BROKER_BINARY)
	@# Both bundles run an executable named oga-desktop, so this quits whichever
	@# Oga is open — released or local. One broker owns port 7331 and ~/.oga, so
	@# the one being installed has to be the only one running.
	pkill -x oga-desktop || true
	# The broker outlives the app it was spawned from, and the next launch finds
	# port 7331 already answering /health — so it reports healthy while serving
	# the previous build's contract. Retire it with the app.
	pkill -f 'Contents/Resources/oga-server' || true
	@listeners=$$(lsof -t -nP -iTCP:7331 -sTCP:LISTEN 2>/dev/null || true); \
	if [ -n "$$listeners" ]; then kill $$listeners || true; fi
	@if [ "$(CUTOVER_SKIP_BACKUP)" = "1" ]; then \
		test "$(CUTOVER_RESTORE)" = "1" || { echo "install: refusing to skip the database backup"; exit 2; }; \
	else \
		bash rust/packaging/cutover.sh backup --database "$(OGA_DB)" --backup "$(CUTOVER_BACKUP)"; \
	fi
	@if [ "$(CUTOVER_RESTORE)" = "1" ]; then \
		bash rust/packaging/cutover.sh restore --database "$(OGA_DB)" --backup "$(CUTOVER_BACKUP)"; \
	fi
	mkdir -p $(INSTALL_DIR)
	rm -rf "$(INSTALL_APP)"
	ditto $(APP) "$(INSTALL_APP)"
	@# Renaming the copy and its identifier is what keeps a released Oga.app out
	@# of this install's way, in Applications and in LaunchServices alike.
	bash scripts/localize-app.sh "$(INSTALL_APP)" "$(LOCAL_APP_NAME)" "$(DEVELOPER_ID_APP)" rust/apps/oga-desktop/entitlements.plist
	@# An Oga.app left in $(INSTALL_DIR) by an install that predates this one
	@# still carries the release identifier, so it competes with the released
	@# app for every launch. It is no longer written to, and can be deleted.
	@test ! -d "$(INSTALL_DIR)/Oga.app" || \
		echo "install: note: $(INSTALL_DIR)/Oga.app is from an older install and is no longer updated; delete it"
	@# The bundle is the app, not the CLI: link the broker binary onto PATH so
	@# `oga` names this install — the development build, not a released one.
	@# BINDIR overrides where the link lands, and a link directory missing from
	@# PATH is a warning, never a failure.
	mkdir -p $(BINDIR)
	ln -sf "$(INSTALL_APP)/Contents/Resources/oga-server" $(BINDIR)/oga
	@case ":$$PATH:" in \
		*":$(BINDIR):"*) ;; \
		*) echo "install: warning: $(BINDIR) is not on PATH; add it to your shell profile for the oga command" ;; \
	esac
	@# open(1) returning is not success: a cold launch takes seconds to answer,
	@# and a broker that survived the pkill answers with the previous build's
	@# contract. Success is the port answering with exactly what the binary
	@# just built reports; anything else fails the install loudly. When open(1)
	@# itself refuses (no GUI session to ask — over SSH, say), the app is on
	@# disk, the broker check is skipped, and the message says how to open it.
	@launched=1; \
	open "$(INSTALL_APP)" || launched=0; \
	if [ "$$launched" -eq 0 ]; then \
		echo "install: $(LOCAL_APP_NAME) is installed at $(INSTALL_APP) but could not be launched"; \
		echo "install: open(1) had no GUI session to ask — typical over SSH or in a session without one"; \
		echo "install: open $(LOCAL_APP_NAME) from Applications; the broker check was skipped"; \
	else \
		health=""; \
		for i in $$(seq 1 30); do \
			health=$$(curl -sf --max-time 2 http://127.0.0.1:7331/health 2>/dev/null) && break; \
			sleep 1; \
		done; \
		if [ -z "$$health" ]; then \
			echo "install: FAILED: the app was launched but no broker answered /health on port 7331 within 30s"; \
			echo "install: open $(LOCAL_APP_NAME) from Applications and check whether the broker comes up"; \
			pkill -f 'Contents/Resources/oga-server' || true; \
			bash rust/packaging/cutover.sh restore --database "$(OGA_DB)" --backup "$(CUTOVER_BACKUP)"; \
			exit 1; \
		fi; \
		built=$$($(BROKER_BINARY) version); \
		if [ "$$health" != "$$built" ]; then \
			echo "install: FAILED: the broker on port 7331 is not the build just installed"; \
			echo "  /health answers: $$health"; \
			echo "  just built:      $$built"; \
			echo "install: a released Oga may have reopened and taken the port; quit it and run make install again"; \
			pkill -f 'Contents/Resources/oga-server' || true; \
			bash rust/packaging/cutover.sh restore --database "$(OGA_DB)" --backup "$(CUTOVER_BACKUP)"; \
			exit 1; \
		fi; \
		echo "install: broker verified — $$health"; \
		if [ -S "$$HOME/.oga/oga.sock" ]; then \
			echo "install: event socket bound — $$HOME/.oga/oga.sock"; \
		else \
			echo "install: warning: no event socket at $$HOME/.oga/oga.sock; watch will fall back to database polling (harmless, but push is off)"; \
		fi; \
	fi

# Local asset links get a version query so the edge cache cannot serve an
# older stylesheet or image next to new HTML.
changelog:
	bun scripts/changelog-to-html.mjs

deploy-landing: changelog
	rm -rf $(DIST)/landing
	mkdir -p $(DIST)
	cp -R landing $(DIST)/landing
	sed -i '' -E 's/(src|srcset|href)="([A-Za-z0-9_./-]+\.(css|png|svg))"/\1="\2?v=$(BUILD_STAMP)"/g' $(DIST)/landing/index.html
	bunx wrangler pages deploy $(DIST)/landing --project-name oga

sync-version:
	@test -n "$(VERSION)" || { echo "error: VERSION file missing"; exit 1; }
	@python3 -c 'import re; from pathlib import Path; version="$(VERSION)"; cargo=Path("rust/Cargo.toml"); text=cargo.read_text(); text=re.sub(r"(\[workspace\.package\][\s\S]*?version = )\"[^\"]+\"", lambda m: m.group(1) + "\"" + version + "\"", text, count=1); cargo.write_text(text); config=Path("rust/apps/oga-desktop/tauri.conf.json"); text=config.read_text(); config.write_text(re.sub(r"(\"version\"\s*:\s*)\"[^\"]+\"", lambda m: m.group(1) + "\"" + version + "\"", text, count=1))'
	@cargo update --manifest-path $(RUST_MANIFEST) --workspace --offline --quiet
	@printf "synced Oga to %s\n" "$(VERSION)"

check-publish-tools:
	@command -v aws >/dev/null 2>&1 || { echo "error: aws CLI not found"; exit 1; }
	@command -v gh >/dev/null 2>&1 || { echo "error: gh CLI not found"; exit 1; }
	@command -v op >/dev/null 2>&1 || { echo "error: 1Password CLI (op) not found"; exit 1; }
	@op run --env-file=.env.1password -- true || { echo "error: failed to resolve the Oga release secrets"; exit 1; }

# Bumps VERSION, commits, then builds, uploads to R2, and tags.
publish:
	@bash scripts/bump-version.sh $(filter-out publish,$(MAKECMDGOALS))
	@$(MAKE) --no-print-directory _publish

# Publishes the VERSION already committed, without a bump.
release: _publish

major minor fix:
	@:

_publish: check-publish-tools sync-version
	@bash scripts/publish.sh
	@$(MAKE) --no-print-directory deploy-landing

clean:
	rm -rf $(DIST)
	cargo clean --manifest-path $(RUST_MANIFEST)
