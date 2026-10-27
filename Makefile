SHELL := /bin/bash

# Node is provided by vite-plus in this environment; harmless on machines where it doesn't exist.
NODE_BIN ?= /root/.local/share/vite-plus/bin
export PATH := $(NODE_BIN):$(PATH)

.PHONY: all web build run dev test fmt clippy lint docker clean

all: build

web:
	cd web && npm install
	cd web && npm run build

build: web
	cargo build --release

run:
	cargo run -- serve

dev:
	@echo "Backend:  cargo run -- serve"
	@echo "Frontend: cd web && npm run dev"
	@echo "Vite proxies /api, /v1, /healthz and /readyz to 127.0.0.1:3000."

test:
	cargo test

fmt:
	cargo fmt --all

clippy:
	cargo clippy --all-targets -- -D warnings

lint: fmt clippy

docker:
	docker build -t oneapi:latest .

clean:
	cargo clean
	rm -rf web/dist
