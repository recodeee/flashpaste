# flashpaste — top-level make targets.
#
# Useful targets:
#   make deb           build .deb package into dist/
#   make install       run install.sh
#   make doctor        run environment check
#   make uninstall     remove symlinks (best-effort)
#   make clean         remove dist/
#   make release-deb   tag VERSION + build .deb + (manual) gh release upload

REPO_ROOT := $(shell pwd)
VERSION ?= $(shell git describe --tags --abbrev=0 2>/dev/null | sed 's/^v//')
VERSION := $(if $(VERSION),$(VERSION),0.1.0)

.PHONY: deb install doctor uninstall clean release-deb help bench bench-tier1 bench-tier3 bench-markdown

help:
	@echo "flashpaste — make targets:"
	@echo "  make deb           build .deb (VERSION=$(VERSION))"
	@echo "  make install       run install.sh (symlinks into ~/.local/bin)"
	@echo "  make doctor        environment check"
	@echo "  make uninstall     remove symlinks"
	@echo "  make clean         delete dist/"
	@echo "  make release-deb   build .deb ready for GitHub Releases"
	@echo "  make bench         dispatch latency benchmark (all tiers)"
	@echo "  make bench-tier1   bench Tier 1 only"
	@echo "  make bench-tier3   bench Tier 3 only"
	@echo "  make bench-markdown bench all tiers, markdown table output"

deb:
	VERSION=$(VERSION) bash packaging/build-deb.sh

install:
	bash install.sh

doctor:
	bash bin/flashpaste-doctor.sh

uninstall:
	@echo "Removing flashpaste symlinks from ~/.local/bin and ~/.local/share/applications"
	@for f in tmux-paste-dispatch.sh clipboard-set.sh clipboard-janitor.sh \
	          get-clipboard-text.sh clip-pipeline-log.sh screenshot-to-clipboard \
	          flashpaste-screenshot-preload.sh flashpaste-doctor.sh \
	          flashpaste-trace.sh wl-paste; do \
	  [ -L "$$HOME/.local/bin/$$f" ] && rm "$$HOME/.local/bin/$$f" && echo "  removed ~/.local/bin/$$f" || true; \
	done
	@for f in wl-clipboard.desktop wl-paste.desktop wl-copy.desktop; do \
	  [ -L "$$HOME/.local/share/applications/$$f" ] && rm "$$HOME/.local/share/applications/$$f" && echo "  removed ~/.local/share/applications/$$f" || true; \
	done
	@[ -L "$$HOME/paste_image.sh" ] && rm "$$HOME/paste_image.sh" && echo "  removed ~/paste_image.sh" || true
	@systemctl --user disable --now clipboard-janitor.service flashpaste-screenshot-watcher.path 2>/dev/null || true
	@echo "uninstall done"

clean:
	rm -rf dist/

release-deb: deb
	@echo "release-deb produced: dist/flashpaste_$(VERSION)_all.deb"
	@echo "Upload to GitHub Releases:"
	@echo "  gh release create v$(VERSION) dist/flashpaste_$(VERSION)_all.deb --title 'flashpaste v$(VERSION)' --generate-notes"

# ---------------------------------------------------------------------------
# Benchmarks
# ---------------------------------------------------------------------------

bench: ## Run the dispatch latency benchmark (100 iterations, all tiers)
	@bash bin/flashpaste-bench.sh

bench-tier1: ## Bench Tier 1 only (bash hot path)
	@bash bin/flashpaste-bench.sh --tier 1

bench-tier3: ## Bench Tier 3 only (daemon + trigger)
	@bash bin/flashpaste-bench.sh --tier 3

bench-markdown: ## Bench all tiers, emit a markdown table
	@bash bin/flashpaste-bench.sh --format markdown
