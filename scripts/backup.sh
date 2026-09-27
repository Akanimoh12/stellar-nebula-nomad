#!/bin/bash
# Automated Backup Script for Stellar Nebula Nomad Contract
# Performs daily automated backups of contract state

set -e

# Configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
BACKUP_DIR="${BACKUP_DIR:-$PROJECT_ROOT/backups}"
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
BACKUP_NAME="nebula_backup_${TIMESTAMP}"
mkdir -p "$BACKUP_DIR"
LOG_FILE="${BACKUP_DIR}/backup.log"

SCHEDULE=false
TEST_RESTORE=false

for arg in "$@"; do
    case "$arg" in
        --schedule) SCHEDULE=true ;;
        --test-restore) TEST_RESTORE=true ;;
    esac
done

if [ "$SCHEDULE" = true ]; then
    echo "Setting up daily cron schedule for backup script..."
    (crontab -l 2>/dev/null | grep -v "$SCRIPT_DIR/backup.sh"; echo "0 2 * * * $SCRIPT_DIR/backup.sh") | crontab -
    echo "Cron schedule configured to run daily at 2:00 AM."
    exit 0
fi

# Stellar configuration
NETWORK="${STELLAR_NETWORK:-testnet}"
CONTRACT_ID="${CONTRACT_ID:-}"
RPC_URL="${RPC_URL:-https://soroban-testnet.stellar.org}"
if [ "$NETWORK" = "mainnet" ] || [ "$NETWORK" = "pubnet" ]; then
    HORIZON_URL="${HORIZON_URL:-https://horizon.stellar.org}"
else
    HORIZON_URL="${HORIZON_URL:-https://horizon-testnet.stellar.org}"
fi
if [ -z "$HORIZON_URL" ]; then
    HORIZON_URL="https://horizon-testnet.stellar.org"
fi

# Backup retention (days)
RETENTION_DAYS="${RETENTION_DAYS:-30}"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Logging functions
log() {
    echo "[$(date +'%Y-%m-%d %H:%M:%S')] $1" | tee -a "$LOG_FILE"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1" | tee -a "$LOG_FILE"
}

log_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1" | tee -a "$LOG_FILE"
}

log_warning() {
    echo -e "${YELLOW}[WARNING]${NC} $1" | tee -a "$LOG_FILE"
}

horizon_get() {
    local path="$1"
    local out="$2"
    if command -v curl >/dev/null 2>&1; then
        curl -fsS -H "Accept: application/json" "${HORIZON_URL}${path}" -o "$out"
    else
        log_error "curl is required to snapshot Horizon state"
        return 1
    fi
}

# Check prerequisites
check_prerequisites() {
    log "Checking prerequisites..."

    if ! command -v curl &> /dev/null; then
        log_error "curl not found. Please install it first."
        exit 1
    fi

    if [ -z "$CONTRACT_ID" ]; then
        log_warning "CONTRACT_ID is not set; capturing ledger-only Horizon snapshot"
    fi

    log "Horizon endpoint: $HORIZON_URL"
    log_success "Prerequisites check passed"
}

# Create backup directory structure
setup_backup_dir() {
    log "Setting up backup directory..."
    
    mkdir -p "$BACKUP_DIR/$BACKUP_NAME"
    mkdir -p "$BACKUP_DIR/$BACKUP_NAME/state"
    mkdir -p "$BACKUP_DIR/$BACKUP_NAME/metadata"
    mkdir -p "$BACKUP_DIR/$BACKUP_NAME/verification"
    
    log_success "Backup directory created: $BACKUP_DIR/$BACKUP_NAME"
}

# Export contract state via the Stellar Horizon API
export_contract_state() {
    log "Exporting contract state from Horizon..."

    local state_file="$BACKUP_DIR/$BACKUP_NAME/state/contract_state.json"
    local ledger_file="$BACKUP_DIR/$BACKUP_NAME/state/latest_ledger.json"

    if ! horizon_get "/ledgers?order=desc&limit=1" "$ledger_file"; then
        log_error "Failed to fetch latest ledger from Horizon ($HORIZON_URL)"
        return 1
    fi

    {
        echo "{"
        echo "  \"horizon_url\": \"$HORIZON_URL\","
        echo "  \"network\": \"$NETWORK\","
        echo "  \"contract_id\": \"$CONTRACT_ID\","
        echo "  \"captured_at\": \"$(date -u +%Y-%m-%dT%H:%M:%SZ)\","
        echo "  \"latest_ledger\": $(cat "$ledger_file")"
        echo "}"
    } > "$state_file"

    if [ -n "$CONTRACT_ID" ]; then
        local contract_file="$BACKUP_DIR/$BACKUP_NAME/state/horizon_contract.json"
        if horizon_get "/contracts/${CONTRACT_ID}" "$contract_file"; then
            log_success "Horizon /contracts snapshot saved"
        elif horizon_get "/accounts/${CONTRACT_ID}" "$contract_file"; then
            log_success "Horizon /accounts snapshot saved"
        else
            log_warning "Horizon has no /contracts or /accounts record for $CONTRACT_ID"
            echo '{"warning":"contract not found on Horizon"}' > "$contract_file"
        fi
    fi

    log_success "Contract state exported to $state_file"
}

# Export contract metadata
export_metadata() {
    log "Exporting contract metadata..."
    
    local metadata_file="$BACKUP_DIR/$BACKUP_NAME/metadata/contract_info.json"
    
    cat > "$metadata_file" <<EOF
{
  "backup_timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "contract_id": "$CONTRACT_ID",
  "network": "$NETWORK",
  "rpc_url": "$RPC_URL",
  "backup_version": "1.0",
  "backup_name": "$BACKUP_NAME"
}
EOF
    
    log_success "Metadata exported to $metadata_file"
}

# Export contract WASM
export_contract_wasm() {
    log "Exporting contract WASM..."

    local wasm_file="$BACKUP_DIR/$BACKUP_NAME/state/contract.wasm"

    if command -v stellar &> /dev/null && [ -n "$CONTRACT_ID" ]; then
        stellar contract fetch \
            --id "$CONTRACT_ID" \
            --network "$NETWORK" \
            --rpc-url "$RPC_URL" \
            --out-file "$wasm_file" 2>&1 || {
            log_warning "Failed to export contract WASM (may not be supported)"
            return 0
        }
        log_success "Contract WASM exported to $wasm_file"
        return 0
    fi

    log_warning "Stellar CLI not available; skipping WASM fetch"
}

# Create state snapshots for key data
create_state_snapshots() {
    log "Creating Horizon state snapshots..."

    local snapshot_dir="$BACKUP_DIR/$BACKUP_NAME/state/snapshots"
    mkdir -p "$snapshot_dir"

    horizon_get "/ledgers?order=desc&limit=20" "$snapshot_dir/recent_ledgers.json" \
        || log_warning "Failed to export recent ledgers"

    if [ -n "$CONTRACT_ID" ]; then
        horizon_get "/accounts/${CONTRACT_ID}/effects?order=desc&limit=200" \
            "$snapshot_dir/player_profiles.json" \
            || log_warning "Failed to export player/account effects"
        horizon_get "/accounts/${CONTRACT_ID}/operations?order=desc&limit=200" \
            "$snapshot_dir/leaderboard.json" \
            || log_warning "Failed to export operations snapshot"
        horizon_get "/accounts/${CONTRACT_ID}/transactions?order=desc&limit=50" \
            "$snapshot_dir/global_stats.json" \
            || log_warning "Failed to export transaction snapshot"
    else
        echo '{"note":"CONTRACT_ID unset"}' > "$snapshot_dir/player_profiles.json"
        echo '{"note":"CONTRACT_ID unset"}' > "$snapshot_dir/leaderboard.json"
        echo '{"note":"CONTRACT_ID unset"}' > "$snapshot_dir/global_stats.json"
    fi

    cat > "$snapshot_dir/retention_policy.json" <<EOF
{
  "retention_days": $RETENTION_DAYS,
  "schedule": "daily 02:00 UTC",
  "horizon_url": "$HORIZON_URL"
}
EOF

    log_success "State snapshots created"
}

# Calculate checksums for verification
calculate_checksums() {
    log "Calculating checksums..."
    
    local checksum_file="$BACKUP_DIR/$BACKUP_NAME/verification/checksums.txt"
    
    cd "$BACKUP_DIR/$BACKUP_NAME"
    find . -type f ! -path './verification/checksums.txt' -print0 \
        | sort -z \
        | xargs -0 sha256sum > "$checksum_file"
    cd - > /dev/null
    
    log_success "Checksums calculated and saved to $checksum_file"
}

# Compress backup
compress_backup() {
    log "Compressing backup..."
    
    local archive_file="$BACKUP_DIR/${BACKUP_NAME}.tar.gz"
    
    tar -czf "$archive_file" -C "$BACKUP_DIR" "$BACKUP_NAME" || {
        log_error "Failed to compress backup"
        return 1
    }
    
    # Remove uncompressed directory
    rm -rf "$BACKUP_DIR/$BACKUP_NAME"
    
    local size=$(du -h "$archive_file" | cut -f1)
    log_success "Backup compressed to $archive_file (Size: $size)"
}

# Upload to off-chain storage (optional)
upload_to_storage() {
    log "Uploading to off-chain storage..."
    
    local archive_file="$BACKUP_DIR/${BACKUP_NAME}.tar.gz"
    
    # S3 upload (if configured)
    if [ -n "$S3_BUCKET" ]; then
        log "Uploading to S3 bucket: $S3_BUCKET"
        aws s3 cp "$archive_file" "s3://$S3_BUCKET/backups/" || {
            log_warning "Failed to upload to S3"
            return 0
        }
        log_success "Uploaded to S3"
    fi
    
    # IPFS upload (if configured)
    if command -v ipfs &> /dev/null && [ "$IPFS_UPLOAD" = "true" ]; then
        log "Uploading to IPFS..."
        local ipfs_hash=$(ipfs add -Q "$archive_file")
        echo "$ipfs_hash" > "$BACKUP_DIR/${BACKUP_NAME}_ipfs.txt"
        log_success "Uploaded to IPFS: $ipfs_hash"
    fi
}

# Clean old backups
cleanup_old_backups() {
    log "Cleaning up old backups (retention: $RETENTION_DAYS days)..."
    
    find "$BACKUP_DIR" -name "nebula_backup_*.tar.gz" -type f -mtime +$RETENTION_DAYS -delete
    
    local remaining=$(find "$BACKUP_DIR" -name "nebula_backup_*.tar.gz" -type f | wc -l)
    log_success "Cleanup complete. Remaining backups: $remaining"
}

# Verify backup integrity
verify_backup() {
    log "Verifying backup integrity..."
    
    local archive_file="$BACKUP_DIR/${BACKUP_NAME}.tar.gz"
    
    # Test archive integrity
    tar -tzf "$archive_file" > /dev/null 2>&1 || {
        log_error "Backup archive is corrupted!"
        return 1
    }
    
    # Extract and verify checksums
    local temp_dir=$(mktemp -d)
    tar -xzf "$archive_file" -C "$temp_dir"
    
    cd "$temp_dir/$BACKUP_NAME"
    if ! sha256sum -c verification/checksums.txt; then
        log_error "Checksum verification failed!"
        rm -rf "$temp_dir"
        return 1
    fi
    cd - > /dev/null
    
    rm -rf "$temp_dir"
    
    log_success "Backup integrity verified"
}

# Send notification
send_notification() {
    local status=$1
    local message=$2
    
    if [ -n "$WEBHOOK_URL" ]; then
        curl -X POST "$WEBHOOK_URL" \
            -H "Content-Type: application/json" \
            -d "{\"status\": \"$status\", \"message\": \"$message\", \"backup\": \"$BACKUP_NAME\"}" \
            > /dev/null 2>&1 || log_warning "Failed to send notification"
    fi
}

# Main backup process
main() {
    log "=========================================="
    log "Starting backup process: $BACKUP_NAME"
    log "=========================================="
    
    local start_time=$(date +%s)
    
    # Execute backup steps
    check_prerequisites
    setup_backup_dir
    export_contract_state
    export_metadata
    export_contract_wasm
    create_state_snapshots
    calculate_checksums
    compress_backup
    verify_backup
    upload_to_storage
    cleanup_old_backups
    
    if [ "$TEST_RESTORE" = true ]; then
        log "Running automated restore test..."
        if [ -x "$SCRIPT_DIR/restore.sh" ]; then
            bash "$SCRIPT_DIR/restore.sh" --backup "$BACKUP_DIR/${BACKUP_NAME}.tar.gz" --test-mode || {
                log_error "Automated restore test failed!"
                return 1
            }
            log_success "Automated restore test passed."
        else
            log_warning "restore.sh not found or not executable. Skipping restore test."
        fi
    fi
    
    local end_time=$(date +%s)
    local duration=$((end_time - start_time))
    
    log "=========================================="
    log_success "Backup completed successfully in ${duration}s"
    log "Backup location: $BACKUP_DIR/${BACKUP_NAME}.tar.gz"
    log "=========================================="
    
    send_notification "success" "Backup completed successfully in ${duration}s"
}

# Error handling
trap 'log_error "Backup failed with error"; send_notification "error" "Backup failed"; exit 1' ERR

# Run main process
main "$@"
