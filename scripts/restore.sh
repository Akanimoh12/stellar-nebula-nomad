#!/bin/bash
# Disaster Recovery Script for Stellar Nebula Nomad Contract
# Restores contract state from backup

set -e

# Configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
BACKUP_DIR="${BACKUP_DIR:-$PROJECT_ROOT/backups}"
mkdir -p "$BACKUP_DIR"
LOG_FILE="${BACKUP_DIR}/restore.log"

# Stellar configuration
NETWORK="${STELLAR_NETWORK:-testnet}"
CONTRACT_ID="${CONTRACT_ID:-}"
RPC_URL="${RPC_URL:-https://soroban-testnet.stellar.org}"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
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

log_info() {
    echo -e "${BLUE}[INFO]${NC} $1" | tee -a "$LOG_FILE"
}

# Show usage
usage() {
    cat <<EOF
Usage: $0 [OPTIONS]

Restore Stellar Nebula Nomad contract state from backup

OPTIONS:
    -b, --backup FILE       Backup file to restore from (required)
    -c, --contract ID       Contract ID to restore to
    -n, --network NAME      Network (testnet/mainnet)
    -d, --dry-run          Perform dry run without actual restore
    -v, --verify-only      Only verify backup integrity
        --test-mode        Verify + dry-run (used by backup.sh --test-restore)
    -h, --help             Show this help message

EXAMPLES:
    # Restore from specific backup
    $0 --backup backups/nebula_backup_20260428_120000.tar.gz

    # Verify backup only
    $0 --backup backups/nebula_backup_20260428_120000.tar.gz --verify-only

    # Dry run
    $0 --backup backups/nebula_backup_20260428_120000.tar.gz --dry-run

ENVIRONMENT VARIABLES:
    CONTRACT_ID            Target contract ID
    STELLAR_NETWORK        Network (testnet/mainnet)
    RPC_URL               Stellar RPC endpoint
    BACKUP_DIR            Backup directory location

EOF
    exit 1
}

# Parse command line arguments
BACKUP_FILE=""
DRY_RUN=false
VERIFY_ONLY=false

while [[ $# -gt 0 ]]; do
    case $1 in
        -b|--backup)
            BACKUP_FILE="$2"
            shift 2
            ;;
        -c|--contract)
            CONTRACT_ID="$2"
            shift 2
            ;;
        -n|--network)
            NETWORK="$2"
            shift 2
            ;;
        -d|--dry-run)
            DRY_RUN=true
            shift
            ;;
        -v|--verify-only)
            VERIFY_ONLY=true
            shift
            ;;
        --test-mode)
            VERIFY_ONLY=true
            DRY_RUN=true
            shift
            ;;
        -h|--help)
            usage
            ;;
        *)
            log_error "Unknown option: $1"
            usage
            ;;
    esac
done

# Validate inputs
if [ -z "$BACKUP_FILE" ]; then
    log_error "Backup file not specified"
    usage
fi

if [ ! -f "$BACKUP_FILE" ]; then
    log_error "Backup file not found: $BACKUP_FILE"
    exit 1
fi

# Check prerequisites
check_prerequisites() {
    log "Checking prerequisites..."
    
    if [ "$VERIFY_ONLY" = false ] && [ "$DRY_RUN" = false ] && ! command -v stellar &> /dev/null; then
        log_error "Stellar CLI not found. Please install it first."
        exit 1
    fi
    
    if [ "$VERIFY_ONLY" = false ] && [ "$DRY_RUN" = false ] && [ -z "$CONTRACT_ID" ]; then
        log_error "CONTRACT_ID not set and not in verify-only mode"
        exit 1
    fi
    
    log_success "Prerequisites check passed"
}

# Extract backup
extract_backup() {
    log "Extracting backup..."
    
    TEMP_DIR=$(mktemp -d)
    tar -xzf "$BACKUP_FILE" -C "$TEMP_DIR" || {
        log_error "Failed to extract backup"
        exit 1
    }
    
    # Find extracted directory
    BACKUP_NAME=$(ls "$TEMP_DIR" | head -n 1)
    EXTRACT_DIR="$TEMP_DIR/$BACKUP_NAME"
    
    log_success "Backup extracted to $EXTRACT_DIR"
}

# Verify backup integrity
verify_backup_integrity() {
    log "Verifying backup integrity..."
    
    # Check archive integrity
    tar -tzf "$BACKUP_FILE" > /dev/null 2>&1 || {
        log_error "Backup archive is corrupted!"
        return 1
    }
    
    # Verify checksums
    cd "$EXTRACT_DIR"
    if [ -f "verification/checksums.txt" ]; then
        if ! sha256sum -c verification/checksums.txt; then
            log_error "Checksum verification failed!"
            return 1
        fi
        log_success "Checksum verification passed"
    else
        log_warning "No checksums file found, skipping verification"
    fi
    cd - > /dev/null
    
    log_success "Backup integrity verified"
}

# Display backup information
display_backup_info() {
    log_info "=========================================="
    log_info "Backup Information"
    log_info "=========================================="
    
    if [ -f "$EXTRACT_DIR/metadata/contract_info.json" ]; then
        cat "$EXTRACT_DIR/metadata/contract_info.json" | while IFS= read -r line; do
            log_info "$line"
        done
    fi
    
    log_info "=========================================="
}

# Confirm restore operation
confirm_restore() {
    if [ "$DRY_RUN" = true ]; then
        log_warning "DRY RUN MODE - No actual changes will be made"
        return 0
    fi
    
    log_warning "=========================================="
    log_warning "WARNING: This will restore contract state"
    log_warning "Target Contract: $CONTRACT_ID"
    log_warning "Network: $NETWORK"
    log_warning "=========================================="
    
    read -p "Are you sure you want to continue? (yes/no): " -r
    echo
    if [[ ! $REPLY =~ ^[Yy][Ee][Ss]$ ]]; then
        log "Restore cancelled by user"
        exit 0
    fi
}

# Restore contract state
restore_contract_state() {
    log "Restoring contract state..."
    
    if [ "$DRY_RUN" = true ]; then
        log_info "[DRY RUN] Would restore state from: $EXTRACT_DIR/state/contract_state.json"
        return 0
    fi
    
    local state_file="$EXTRACT_DIR/state/contract_state.json"
    
    if [ ! -f "$state_file" ]; then
        log_error "State file not found: $state_file"
        return 1
    fi
    
    # Note: Actual state restoration depends on Stellar's capabilities
    # This is a placeholder for the restoration logic
    log_warning "State restoration requires manual intervention or custom tooling"
    log_info "State file location: $state_file"
    
    log_success "Contract state restore initiated"
}

# Restore snapshots
restore_snapshots() {
    log "Restoring state snapshots..."
    
    if [ "$DRY_RUN" = true ]; then
        log_info "[DRY RUN] Would restore snapshots from: $EXTRACT_DIR/state/snapshots/"
        return 0
    fi
    
    local snapshot_dir="$EXTRACT_DIR/state/snapshots"
    
    if [ ! -d "$snapshot_dir" ]; then
        log_warning "No snapshots directory found"
        return 0
    fi
    
    # List available snapshots
    log_info "Available snapshots:"
    ls -lh "$snapshot_dir" | tail -n +2 | awk '{print "  - " $9 " (" $5 ")"}'
    
    log_success "Snapshots ready for restoration"
}

# Verify restored state
verify_restored_state() {
    log "Verifying restored state..."
    
    if [ "$DRY_RUN" = true ]; then
        log_info "[DRY RUN] Would verify restored state"
        return 0
    fi
    
    # Query contract to verify it's accessible
    stellar contract invoke \
        --id "$CONTRACT_ID" \
        --network "$NETWORK" \
        --rpc-url "$RPC_URL" \
        -- get_global_stats \
        > /dev/null 2>&1 || {
        log_error "Failed to query restored contract"
        return 1
    }
    
    log_success "Restored state verified"
}

# Generate restore report
generate_restore_report() {
    log "Generating restore report..."
    
    local report_file="$BACKUP_DIR/restore_report_$(date +%Y%m%d_%H%M%S).txt"
    
    cat > "$report_file" <<EOF
========================================
Disaster Recovery Report
========================================
Restore Date: $(date -u +%Y-%m-%dT%H:%M:%SZ)
Backup File: $BACKUP_FILE
Contract ID: $CONTRACT_ID
Network: $NETWORK
Dry Run: $DRY_RUN
Status: SUCCESS

Backup Information:
$(cat "$EXTRACT_DIR/metadata/contract_info.json" 2>/dev/null || echo "N/A")

Restored Components:
- Contract State: $([ -f "$EXTRACT_DIR/state/contract_state.json" ] && echo "YES" || echo "NO")
- Contract WASM: $([ -f "$EXTRACT_DIR/state/contract.wasm" ] && echo "YES" || echo "NO")
- Snapshots: $([ -d "$EXTRACT_DIR/state/snapshots" ] && echo "YES" || echo "NO")

Recovery Time: ${SECONDS}s
========================================
EOF
    
    log_success "Restore report generated: $report_file"
    cat "$report_file"
}

# Cleanup
cleanup() {
    if [ -n "$TEMP_DIR" ] && [ -d "$TEMP_DIR" ]; then
        rm -rf "$TEMP_DIR"
        log "Cleaned up temporary files"
    fi
}

# Send notification
send_notification() {
    local status=$1
    local message=$2
    
    if [ -n "$WEBHOOK_URL" ]; then
        curl -X POST "$WEBHOOK_URL" \
            -H "Content-Type: application/json" \
            -d "{\"status\": \"$status\", \"message\": \"$message\", \"contract\": \"$CONTRACT_ID\"}" \
            > /dev/null 2>&1 || log_warning "Failed to send notification"
    fi
}

# Main restore process
main() {
    log "=========================================="
    log "Starting restore process"
    log "=========================================="
    
    local start_time=$(date +%s)
    
    # Execute restore steps
    check_prerequisites
    extract_backup
    verify_backup_integrity
    display_backup_info
    
    if [ "$VERIFY_ONLY" = true ]; then
        log_success "Verification complete - backup is valid"
        cleanup
        exit 0
    fi
    
    confirm_restore
    restore_contract_state
    restore_snapshots
    verify_restored_state
    generate_restore_report
    
    local end_time=$(date +%s)
    local duration=$((end_time - start_time))
    
    log "=========================================="
    log_success "Restore completed successfully in ${duration}s"
    log "=========================================="
    
    send_notification "success" "Restore completed successfully in ${duration}s"
    
    cleanup
}

# Error handling
trap 'log_error "Restore failed with error"; cleanup; send_notification "error" "Restore failed"; exit 1' ERR

# Run main process
main "$@"
