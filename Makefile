COMPILER_MANIFEST := compiler/Cargo.toml

.PHONY: build test check clippy clean

build:
	cargo build --manifest-path $(COMPILER_MANIFEST) --workspace

test:
	cargo test --manifest-path $(COMPILER_MANIFEST) --workspace

check:
	cargo check --manifest-path $(COMPILER_MANIFEST) --workspace

clippy:
	cargo clippy --manifest-path $(COMPILER_MANIFEST) --workspace -- -D warnings

clean:
	cargo clean --manifest-path $(COMPILER_MANIFEST)
