.PHONY: build test lint lint-manifests images compose-up compose-down deploy-local e2e-check

build:
	cargo build --release --workspace

test:
	cargo test --workspace

lint:
	cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings

lint-manifests:
	bash scripts/validate-manifests.sh

images:
	docker build -t ghcr.io/taiki3/ebishrimp-rumqttd:0.20.0 docker/rumqttd
	docker build -t ghcr.io/taiki3/ebishrimp-ingester:dev -f services/ingester/Dockerfile .
	docker build -t ghcr.io/taiki3/ebishrimp-dashboard:dev -f services/dashboard/Dockerfile .
	docker build -t ghcr.io/taiki3/ebishrimp-mock-publisher:dev -f services/mock-publisher/Dockerfile .

compose-up:
	docker compose up -d --build

compose-down:
	docker compose down

deploy-local:
	bash scripts/deploy-local.sh

e2e-check:
	bash scripts/mock-e2e-check.sh
