# Included by the phpize Makefile (config.m4): the extension is built by cargo.
all: modules/revolt.so
build-modules: modules/revolt.so

.PHONY: revolt-cargo-build
revolt-cargo-build:
	cd "$(srcdir)" && PHP="$(PHP_EXECUTABLE)" PHP_CONFIG="$(PHP_CONFIG)" "$(CARGO)" build --release --locked -p revolt-ext

modules/revolt.so: revolt-cargo-build
	@mkdir -p modules
	@lib="$(srcdir)/target/release/librevolt.so"; \
	[ -f "$$lib" ] || lib="$(srcdir)/target/release/librevolt.dylib"; \
	cp "$$lib" $@
