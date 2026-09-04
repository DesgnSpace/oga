#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<'EOF'
Usage:
  cutover.sh backup --database <path> --backup <path>
  cutover.sh restore --database <path> --backup <path>
  cutover.sh install --binary <path> --target <path> [--database <path>] [--backup <path>]
  cutover.sh verify --binary <path> --database <path> --port <port>

The backup contains the database and any SQLite WAL or shared-memory sidecars.
Install creates the backup before replacing the target. Restore is explicit.
Verify starts only the requested binary against the requested database.
EOF
}

fail() {
    printf 'cutover: %s\n' "$*" >&2
    exit 1
}

database_default="${OGA_DB:-${HOME:?}/.oga/oga.db}"
operation="${1:-}"
if (($# > 0)); then
    shift
fi

database="$database_default"
backup="${OGA_CUTOVER_BACKUP:-}"
binary=""
target=""
port="${OGA_CUTOVER_PORT:-7331}"

while (($# > 0)); do
    case "$1" in
        --database)
            (($# >= 2)) || fail "--database needs a value"
            database="$2"
            shift 2
            ;;
        --database=*)
            database="${1#*=}"
            shift
            ;;
        --backup)
            (($# >= 2)) || fail "--backup needs a value"
            backup="$2"
            shift 2
            ;;
        --backup=*)
            backup="${1#*=}"
            shift
            ;;
        --binary)
            (($# >= 2)) || fail "--binary needs a value"
            binary="$2"
            shift 2
            ;;
        --binary=*)
            binary="${1#*=}"
            shift
            ;;
        --target)
            (($# >= 2)) || fail "--target needs a value"
            target="$2"
            shift 2
            ;;
        --target=*)
            target="${1#*=}"
            shift
            ;;
        --port)
            (($# >= 2)) || fail "--port needs a value"
            port="$2"
            shift 2
            ;;
        --port=*)
            port="${1#*=}"
            shift
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        *)
            fail "unknown option: $1"
            ;;
    esac
done

[[ -n "$database" ]] || fail "database path is empty"
if [[ -z "$backup" ]]; then
    backup="${database}.cutover-backup"
fi
[[ -n "$backup" ]] || fail "backup path is empty"

canonical_path() {
    local path="$1"
    local parent
    if [[ -d "$path" ]]; then
        (cd -- "$path" && pwd -P)
        return
    fi
    parent="$(cd -- "$(dirname "$path")" && pwd -P)" || fail "path parent does not exist: $path"
    printf '%s/%s\n' "$parent" "$(basename "$path")"
}

validate_backup_path() {
    mkdir -p "$(dirname "$database")" "$(dirname "$backup")"
    local database_path
    local backup_path
    database_path="$(canonical_path "$database")"
    backup_path="$(canonical_path "$backup")"
    if [[ "$backup_path" == "$database_path" || "$backup_path" == "${database_path}-wal" || "$backup_path" == "${database_path}-shm" ]]; then
        fail "backup path must not be the database or one of its sidecars"
    fi
    if [[ "$backup_path" == "/" || "$database_path" == "$backup_path"/* ]]; then
        fail "backup path must not contain the database: $backup"
    fi
}

copy_database_file() {
    local database_file="$1"
    local destination_directory="$2"

    cp -p "$database_file" "$destination_directory/$(basename "$database_file")"
}

backup_database() {
    local temporary
    local database_state=absent
    local wal_state=absent
    local shm_state=absent

    if [[ -e "$database" ]]; then
        [[ -f "$database" ]] || fail "database path is not a regular file: $database"
        database_state=present
    fi
    if [[ -e "${database}-wal" ]]; then
        [[ -f "${database}-wal" ]] || fail "database WAL is not a regular file: ${database}-wal"
        wal_state=present
    fi
    if [[ -e "${database}-shm" ]]; then
        [[ -f "${database}-shm" ]] || fail "database shared memory file is not a regular file: ${database}-shm"
        shm_state=present
    fi
    if [[ "$database_state" == absent && ("$wal_state" == present || "$shm_state" == present) ]]; then
        fail "database sidecar exists without its database: $database"
    fi

    temporary="$(mktemp -d "${backup}.tmp.XXXXXX")"
    mkdir -p "$(dirname "$backup")"
    if [[ "$database_state" == present ]]; then
        copy_database_file "$database" "$temporary"
    fi
    if [[ "$wal_state" == present ]]; then
        copy_database_file "${database}-wal" "$temporary"
    fi
    if [[ "$shm_state" == present ]]; then
        copy_database_file "${database}-shm" "$temporary"
    fi
    {
        printf 'version=1\n'
        printf 'database=%s\n' "$database_state"
        printf 'wal=%s\n' "$wal_state"
        printf 'shm=%s\n' "$shm_state"
    } > "$temporary/manifest"
    rm -rf "$backup"
    mv "$temporary" "$backup"
    printf 'cutover: database backup ready at %s\n' "$backup"
}

restore_database() {
    [[ -f "$backup/manifest" ]] || fail "database backup is missing: $backup"
    local database_state=""
    local wal_state=""
    local shm_state=""
    local key=""
    local value=""
    while IFS='=' read -r key value; do
        case "$key" in
            version)
                [[ "$value" == 1 ]] || fail "unsupported backup version: $value"
                ;;
            database) database_state="$value" ;;
            wal) wal_state="$value" ;;
            shm) shm_state="$value" ;;
            *) fail "invalid backup manifest key: $key" ;;
        esac
    done < "$backup/manifest"
    [[ "$database_state" == present || "$database_state" == absent ]] || fail "invalid database backup state"
    [[ "$wal_state" == present || "$wal_state" == absent ]] || fail "invalid WAL backup state"
    [[ "$shm_state" == present || "$shm_state" == absent ]] || fail "invalid shared-memory backup state"

    if [[ "$database_state" == absent ]]; then
        rm -f "$database" "${database}-wal" "${database}-shm"
        printf 'cutover: restored an absent database state\n'
        return
    fi

    mkdir -p "$(dirname "$database")"
    local suffix=""
    local state=""
    local source=""
    local destination=""
    local temporary=""
    for suffix in "" "-wal" "-shm"; do
        if [[ -z "$suffix" ]]; then
            state="$database_state"
        elif [[ "$suffix" == "-wal" ]]; then
            state="$wal_state"
        else
            state="$shm_state"
        fi
        destination="${database}${suffix}"
        if [[ "$state" == present ]]; then
            source="$backup/$(basename "$destination")"
            [[ -f "$source" ]] || fail "backup file is missing: $source"
            temporary="$(mktemp "$(dirname "$destination")/.oga-restore.XXXXXX")"
            cp -p "$source" "$temporary"
            mv -f "$temporary" "$destination"
        else
            rm -f "$destination"
        fi
    done
    printf 'cutover: database restored from %s\n' "$backup"
}

install_binary() {
    [[ -x "$binary" ]] || fail "binary is not executable: $binary"
    if [[ "$binary" == "$target" ]]; then
        fail "binary and target must be different paths"
    fi
    if ! "$binary" version >/dev/null; then
        fail "binary did not answer the version command: $binary"
    fi
    backup_database
    mkdir -p "$(dirname "$target")"
    local temporary
    temporary="$(mktemp "${target}.XXXXXX")"
    if ! install -m 755 "$binary" "$temporary"; then
        rm -f "$temporary"
        fail "could not stage the binary at $target"
    fi
    mv -f "$temporary" "$target"
    printf 'cutover: installed Rust broker at %s\n' "$target"
}

verify_broker() {
    [[ -x "$binary" ]] || fail "binary is not executable: $binary"
    [[ "$port" =~ ^[0-9]+$ ]] || fail "not a port: $port"
    ((port >= 1 && port <= 65535)) || fail "not a port: $port"
    command -v curl >/dev/null 2>&1 || fail "curl is required for broker verification"
    mkdir -p "$(dirname "$database")"

    local version
    local health=""
    local temporary
    local log
    local log_text
    local server_pid=""
    version="$("$binary" version)" || fail "binary did not answer the version command: $binary"
    temporary="$(mktemp -d "${TMPDIR:-/tmp}/oga-cutover-verify.XXXXXX")"
    log="$temporary/broker.log"
    cleanup_verify() {
        if [[ -n "$server_pid" ]]; then
            kill "$server_pid" 2>/dev/null || true
            wait "$server_pid" 2>/dev/null || true
        fi
        rm -rf "$temporary"
    }
    trap cleanup_verify EXIT INT TERM

    OGA_DB="$database" OGA_PORT="$port" "$binary" serve > "$log" 2>&1 &
    server_pid=$!
    for _ in {1..30}; do
        if health="$(curl -fsS --max-time 2 "http://127.0.0.1:$port/health" 2>/dev/null)"; then
            break
        fi
        if ! kill -0 "$server_pid" 2>/dev/null; then
            break
        fi
        sleep 1
    done
    if [[ -z "$health" ]]; then
        printf 'cutover: broker did not answer /health. Server log:\n' >&2
        log_text="$(<"$log")"
        printf '%s\n' "$log_text" >&2
        exit 1
    fi
    if [[ "$health" != "$version" ]]; then
        printf 'cutover: broker identity mismatch\n' >&2
        printf 'oga version: %s\n/health: %s\n' "$version" "$health" >&2
        exit 1
    fi
    printf 'cutover: broker verified - %s\n' "$health"
    cleanup_verify
    trap - EXIT INT TERM
}

case "$operation" in
    backup)
        validate_backup_path
        backup_database
        ;;
    restore)
        validate_backup_path
        restore_database
        ;;
    install)
        [[ -n "$binary" ]] || fail "--binary is required for install"
        [[ -n "$target" ]] || fail "--target is required for install"
        validate_backup_path
        install_binary
        ;;
    verify)
        [[ -n "$binary" ]] || fail "--binary is required for verify"
        verify_broker
        ;;
    ""|--help|-h)
        usage
        ;;
    *)
        fail "unknown operation: $operation"
        ;;
esac
