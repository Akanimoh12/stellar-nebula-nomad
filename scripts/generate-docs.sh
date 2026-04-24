#!/usr/bin/env bash
# generate-docs.sh
#
# Generates API documentation for Stellar Nebula Nomad from the OpenAPI spec
# and Rust doc comments.
#
# Usage:
#   ./scripts/generate-docs.sh [--serve] [--validate-only]
#
# Options:
#   --serve          Start an interactive Swagger UI server on http://localhost:8080
#   --validate-only  Only validate the OpenAPI spec; do not generate output files
#
# Requirements:
#   - npx (Node.js)           — for @redocly/cli and swagger-ui
#   - cargo                   — for Rust doc generation (rustdoc)
#   - Optional: redoc-cli     — for static HTML generation
#
# Output:
#   docs/api/index.html       — Static Redoc HTML (interactive explorer)
#   docs/api/swagger-ui/      — Swagger UI interactive explorer
#   target/doc/               — Rust API docs (cargo doc output)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
OPENAPI_SPEC="${ROOT_DIR}/docs/api/openapi.yaml"
OUTPUT_DIR="${ROOT_DIR}/docs/api"

# ─── Argument parsing ──────────────────────────────────────────────────────────

SERVE=false
VALIDATE_ONLY=false

for arg in "$@"; do
  case "$arg" in
    --serve)         SERVE=true ;;
    --validate-only) VALIDATE_ONLY=true ;;
    *)
      echo "Unknown option: $arg"
      echo "Usage: $0 [--serve] [--validate-only]"
      exit 1
      ;;
  esac
done

# ─── Helpers ───────────────────────────────────────────────────────────────────

info()  { echo -e "\033[1;34m[INFO]\033[0m  $*"; }
ok()    { echo -e "\033[1;32m[ OK ]\033[0m  $*"; }
warn()  { echo -e "\033[1;33m[WARN]\033[0m  $*"; }
error() { echo -e "\033[1;31m[ERR ]\033[0m  $*" >&2; }

require_command() {
  if ! command -v "$1" &>/dev/null; then
    warn "'$1' not found — skipping $2 step. Install it to enable this feature."
    return 1
  fi
  return 0
}

# ─── Step 1: Validate OpenAPI spec ─────────────────────────────────────────────

info "Validating OpenAPI spec: ${OPENAPI_SPEC}"

if require_command npx "spec validation"; then
  if npx --yes @redocly/cli lint "${OPENAPI_SPEC}" 2>&1; then
    ok "OpenAPI spec is valid."
  else
    error "OpenAPI spec validation failed."
    exit 1
  fi
else
  # Fallback: basic YAML syntax check via Python (usually available on all systems)
  if command -v python3 &>/dev/null; then
    python3 -c "import yaml; yaml.safe_load(open('${OPENAPI_SPEC}'))" \
      && ok "YAML syntax OK (install npx for full OpenAPI validation)" \
      || { error "YAML syntax error in ${OPENAPI_SPEC}"; exit 1; }
  fi
fi

if [ "$VALIDATE_ONLY" = "true" ]; then
  ok "Validation complete (--validate-only mode). Exiting."
  exit 0
fi

# ─── Step 2: Generate static Redoc HTML ────────────────────────────────────────

info "Generating static Redoc API documentation…"

mkdir -p "${OUTPUT_DIR}"

if require_command npx "Redoc HTML generation"; then
  npx --yes @redocly/cli build-docs "${OPENAPI_SPEC}" \
    --output "${OUTPUT_DIR}/index.html" \
    --title "Stellar Nebula Nomad — API Reference" \
    && ok "Redoc HTML written to ${OUTPUT_DIR}/index.html"
else
  warn "Skipped Redoc HTML generation (npx not available)."
fi

# ─── Step 3: Generate Swagger UI ───────────────────────────────────────────────

info "Copying OpenAPI spec for Swagger UI…"

SWAGGER_UI_DIR="${OUTPUT_DIR}/swagger-ui"
mkdir -p "${SWAGGER_UI_DIR}"

# Write a minimal Swagger UI HTML that loads the local openapi.yaml.
cat > "${SWAGGER_UI_DIR}/index.html" <<'HTML'
<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>Stellar Nebula Nomad — Swagger UI</title>
  <link rel="stylesheet" href="https://unpkg.com/swagger-ui-dist/swagger-ui.css" />
</head>
<body>
  <div id="swagger-ui"></div>
  <script src="https://unpkg.com/swagger-ui-dist/swagger-ui-bundle.js"></script>
  <script>
    SwaggerUIBundle({
      url: "../openapi.yaml",
      dom_id: '#swagger-ui',
      presets: [SwaggerUIBundle.presets.apis, SwaggerUIBundle.SwaggerUIStandalonePreset],
      layout: "BaseLayout",
      deepLinking: true,
    });
  </script>
</body>
</html>
HTML

ok "Swagger UI written to ${SWAGGER_UI_DIR}/index.html"

# ─── Step 4: Generate Rust API docs ────────────────────────────────────────────

info "Generating Rust documentation (cargo doc)…"

if require_command cargo "Rust doc generation"; then
  cd "${ROOT_DIR}"
  cargo doc --no-deps --document-private-items 2>&1 \
    && ok "Rust docs written to target/doc/" \
    || warn "cargo doc encountered warnings/errors (check output above)."
else
  warn "Skipped Rust doc generation (cargo not available)."
fi

# ─── Step 5: Serve (optional) ─────────────────────────────────────────────────

if [ "$SERVE" = "true" ]; then
  info "Starting interactive Swagger UI server on http://localhost:8080 …"
  info "Press Ctrl+C to stop."

  if require_command npx "dev server"; then
    cd "${OUTPUT_DIR}"
    npx --yes serve . --listen 8080
  elif command -v python3 &>/dev/null; then
    cd "${OUTPUT_DIR}"
    python3 -m http.server 8080
  else
    error "No suitable HTTP server found. Install npx or python3."
    exit 1
  fi
fi

# ─── Summary ──────────────────────────────────────────────────────────────────

echo ""
ok "Documentation generation complete."
echo ""
echo "  Interactive API explorer:  ${OUTPUT_DIR}/index.html"
echo "  Swagger UI:                ${SWAGGER_UI_DIR}/index.html"
echo "  Rust docs:                 ${ROOT_DIR}/target/doc/stellar_nebula_nomad/index.html"
echo ""
echo "  To validate only:  $0 --validate-only"
echo "  To serve locally:  $0 --serve"
