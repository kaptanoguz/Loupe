# Loupe — geliştirme, test ve Linux paketleme (RFC-001G §4).
#
#   make dev       arayüz + Tauri, sıcak yeniden yükleme (ONNX ve libvips açık)
#   make build     sürüm ikilisi: target/release/loupe-photos
#   make bundle    .deb ve .AppImage: target/release/bundle/{deb,appimage}/
#   make models    AI modellerini indir ve doğrula (~/.local/share/loupe/models)
#   make test      CI'daki denetimlerin tamamı
#   make mobile    telefon sayfasını gerçek tarayıcıda denemek için test sunucusu
#
# İlk kurulum (Debian/Ubuntu): make deps

SHELL    := bash
DESKTOP  := apps/desktop
FEATURES ?= ort,vips
CARGO    ?= cargo
NPM      ?= npm

.PHONY: help deps node dev build bundle models models-status test lint fmt rust-test web-test mobile clean

help:
	@sed -n 's/^#   //p' Makefile

deps:
	sudo apt-get install -y build-essential curl pkg-config libssl-dev \
	  libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev \
	  libvips-tools file
	@echo "Rust: https://rustup.rs · Node 22+: https://nodejs.org"

node: $(DESKTOP)/node_modules/.package-lock.json

$(DESKTOP)/node_modules/.package-lock.json: $(DESKTOP)/package-lock.json
	cd $(DESKTOP) && $(NPM) ci

dev: node
	cd $(DESKTOP) && $(NPM) run tauri -- dev --features $(FEATURES)

build: node
	cd $(DESKTOP) && $(NPM) run tauri -- build --no-bundle --features $(FEATURES)
	@echo "→ target/release/loupe-photos"

bundle: node
	cd $(DESKTOP) && $(NPM) run tauri -- build --bundles deb,appimage --features $(FEATURES)
	@ls -1 target/release/bundle/deb/*.deb target/release/bundle/appimage/*.AppImage

models:
	scripts/fetch-models.sh install

models-status:
	scripts/fetch-models.sh status

fmt:
	$(CARGO) fmt --all

lint: node
	$(CARGO) fmt --all -- --check
	$(CARGO) clippy --workspace --all-targets -- -D warnings
	$(CARGO) clippy -p loupe-ai -p loupe-app --all-targets --features ort -- -D warnings
	cd $(DESKTOP) && $(NPM) run check
	bash tools/slop-guard/slop-guard.sh
	bash tools/slop-guard/slop-guard.sh crates/loupe-lan/web

rust-test:
	$(CARGO) test --workspace

web-test: node
	cd $(DESKTOP) && $(NPM) test

test: lint rust-test web-test

mobile:
	LOUPE_SERVE_SECS=$${LOUPE_SERVE_SECS:-600} $(CARGO) test -p loupe-lan --test mobile -- --ignored --nocapture serve_for_browser

clean:
	$(CARGO) clean
	rm -rf $(DESKTOP)/dist
