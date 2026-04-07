#!/usr/bin/env bash
#
# socks5-bench.sh — Lightweight SOCKS5 proxy benchmark (no clash/mihomo dependency)
#
# Usage:
#   ./socks5-bench.sh <nodes-file> [options]
#   echo "socks5://user:pass@host:port" | ./socks5-bench.sh - [options]
#
# Nodes file format (one per line):
#   socks5://user:pass@host:port           # standard
#   socks5h://user:pass@host:port          # remote DNS
#   socks5://host:port                     # no auth
#   # comment lines and empty lines are skipped
#   //Tokyo                                # section headers (shown in output)
#
# Options:
#   -r, --rounds N       Number of test rounds (default: 5)
#   -i, --interval N     Seconds between rounds (default: 10)
#   -t, --timeout N      Timeout per probe in seconds (default: 10)
#   -u, --url URL        Test URL (default: http://www.gstatic.com/generate_204)
#   -c, --concurrency N  Max concurrent probes per round (default: 20)
#   -h, --help           Show this help

set -euo pipefail

# ── Defaults ──
ROUNDS=5
INTERVAL=10
TIMEOUT=10
TEST_URL="http://www.gstatic.com/generate_204"
CONCURRENCY=5
NODES_FILE=""

# ── Colors ──
if [[ -t 1 ]]; then
    GREEN='\033[0;32m'
    YELLOW='\033[0;33m'
    RED='\033[0;31m'
    BOLD='\033[1m'
    DIM='\033[2m'
    RESET='\033[0m'
else
    GREEN="" YELLOW="" RED="" BOLD="" DIM="" RESET=""
fi

usage() {
    sed -n '3,/^$/p' "$0" | sed 's/^# \?//'
    exit 0
}

# ── Parse args ──
while [[ $# -gt 0 ]]; do
    case "$1" in
        -r|--rounds)      ROUNDS="$2"; shift 2 ;;
        -i|--interval)    INTERVAL="$2"; shift 2 ;;
        -t|--timeout)     TIMEOUT="$2"; shift 2 ;;
        -u|--url)         TEST_URL="$2"; shift 2 ;;
        -c|--concurrency) CONCURRENCY="$2"; shift 2 ;;
        -h|--help)        usage ;;
        -*)               echo "Unknown option: $1" >&2; exit 1 ;;
        *)
            if [[ -z "$NODES_FILE" ]]; then
                NODES_FILE="$1"; shift
            else
                echo "Unexpected argument: $1" >&2; exit 1
            fi
            ;;
    esac
done

if [[ -z "$NODES_FILE" ]]; then
    echo "Usage: $0 <nodes-file> [options]" >&2
    echo "       echo 'socks5://host:port' | $0 - [options]" >&2
    exit 1
fi

# ── Read nodes ──
declare -a NODE_NAMES=()
declare -a NODE_URLS=()
current_section=""

while IFS= read -r line; do
    line="${line%%$'\r'}"           # strip CR
    line="${line#"${line%%[![:space:]]*}"}"  # trim leading space
    [[ -z "$line" ]] && continue
    [[ "$line" == \#* ]] && continue
    if [[ "$line" == //* ]]; then
        current_section="${line#//}"
        current_section="${current_section#"${current_section%%[![:space:]]*}"}"
        continue
    fi
    # Extract display name: section + host:port or just the URI
    uri="$line"
    # Normalize socks5h to socks5 for curl
    uri="${uri/socks5h:\/\//socks5h:\/\/}"
    # Extract display name: prefer user part (contains target IP) over host:port
    hostport="${uri#*@}"
    [[ "$hostport" == "$uri" ]] && hostport="${uri#*://}"
    hostport="${hostport%%/*}"
    # For Bright Data style URIs, extract the target IP from username
    local_user="${uri#*://}"
    local_user="${local_user%%@*}"
    local_user="${local_user%%:*}"
    if [[ "$local_user" =~ ip-([0-9.]+)$ ]]; then
        hostport="${BASH_REMATCH[1]}"
    fi
    if [[ -n "$current_section" ]]; then
        name="${current_section}-${hostport}"
    else
        name="$hostport"
    fi
    NODE_NAMES+=("$name")
    NODE_URLS+=("$uri")
done < <(if [[ "$NODES_FILE" == "-" ]]; then cat; else cat "$NODES_FILE"; fi)

NODE_COUNT=${#NODE_NAMES[@]}
if [[ $NODE_COUNT -eq 0 ]]; then
    echo "No nodes found in input." >&2
    exit 1
fi

# ── Temp dir for results ──
TMPDIR_BENCH=$(mktemp -d)
trap 'rm -rf "$TMPDIR_BENCH"' EXIT

# Initialize per-node result files
for idx in $(seq 0 $((NODE_COUNT - 1))); do
    : > "$TMPDIR_BENCH/node_${idx}.csv"
done

# ── Probe function ──
probe_node() {
    local idx=$1 uri=$2 timeout=$3 url=$4
    local start elapsed http_code
    local hard_timeout=$((timeout + 3))
    local connect_timeout=$(( timeout / 2 ))
    [[ $connect_timeout -lt 3 ]] && connect_timeout=3
    start=$(date +%s%N 2>/dev/null || python3 -c 'import time; print(int(time.time()*1e9))')
    http_code=$(timeout "$hard_timeout" curl -s -o /dev/null -w "%{http_code}" \
        --proxy "$uri" \
        --max-time "$timeout" \
        --connect-timeout "$connect_timeout" \
        "$url" 2>/dev/null) || http_code="000"
    local end
    end=$(date +%s%N 2>/dev/null || python3 -c 'import time; print(int(time.time()*1e9))')
    elapsed=$(( (end - start) / 1000000 ))  # ms
    if [[ "$http_code" == "200" || "$http_code" == "204" ]]; then
        echo "$elapsed" >> "$TMPDIR_BENCH/node_${idx}.csv"
    else
        echo "0" >> "$TMPDIR_BENCH/node_${idx}.csv"
    fi
}

# ── Run benchmark ──
printf "${BOLD}Benchmarking %d nodes — %d rounds, %ds interval, %ds timeout${RESET}\n\n" \
    "$NODE_COUNT" "$ROUNDS" "$INTERVAL" "$TIMEOUT"

for round in $(seq 1 "$ROUNDS"); do
    # Launch probes in parallel with concurrency limit
    active=0
    for idx in $(seq 0 $((NODE_COUNT - 1))); do
        probe_node "$idx" "${NODE_URLS[$idx]}" "$TIMEOUT" "$TEST_URL" &
        active=$((active + 1))
        # Stagger launches to avoid proxy rate-limiting
        sleep 0.3
        if [[ $active -ge $CONCURRENCY ]]; then
            wait -n 2>/dev/null || wait
            active=$((active - 1))
        fi
    done
    wait

    # Count results
    ok=0 fail=0
    for idx in $(seq 0 $((NODE_COUNT - 1))); do
        last=$(tail -1 "$TMPDIR_BENCH/node_${idx}.csv")
        if [[ "$last" != "0" ]]; then
            ok=$((ok + 1))
        else
            fail=$((fail + 1))
        fi
    done

    if [[ $round -lt $ROUNDS ]]; then
        printf "\r  [%d/%d] %d ok, %d timeout — next in %ds  " "$round" "$ROUNDS" "$ok" "$fail" "$INTERVAL"
        sleep "$INTERVAL"
    else
        printf "\r  [%d/%d] %d ok, %d timeout                \n" "$round" "$ROUNDS" "$ok" "$fail"
    fi
done

echo ""

# ── Compute stats ──
calc_stats() {
    local file=$1
    local -a samples=()
    while IFS= read -r val; do
        samples+=("$val")
    done < "$file"

    local total=${#samples[@]}
    [[ $total -eq 0 ]] && echo "- - - - - - 100" && return

    # Count losses
    local losses=0 ok_count=0
    local -a ok_vals=()
    for v in "${samples[@]}"; do
        if [[ "$v" == "0" ]]; then
            losses=$((losses + 1))
        else
            ok_vals+=("$v")
            ok_count=$((ok_count + 1))
        fi
    done

    local loss_pct=$((losses * 100 / total))

    if [[ $ok_count -eq 0 ]]; then
        echo "- - - - - - $loss_pct"
        return
    fi

    # Sort ok values
    IFS=$'\n' sorted=($(printf '%s\n' "${ok_vals[@]}" | sort -n)); unset IFS

    # avg
    local sum=0
    for v in "${ok_vals[@]}"; do sum=$((sum + v)); done
    local avg=$((sum / ok_count))

    # min / max
    local min=${sorted[0]}
    local max=${sorted[$((ok_count - 1))]}

    # p95
    local p95_idx=$(( (ok_count * 95 + 99) / 100 - 1 ))
    [[ $p95_idx -ge $ok_count ]] && p95_idx=$((ok_count - 1))
    local p95=${sorted[$p95_idx]}

    # jitter (stddev)
    local jitter="-"
    if [[ $ok_count -ge 2 ]]; then
        jitter=$(python3 -c "
import math
vals = [${ok_vals[*]// /,}]
mean = sum(vals) / len(vals)
var = sum((x - mean) ** 2 for x in vals) / len(vals)
print(int(math.sqrt(var)))
" 2>/dev/null || echo "-")
    fi

    echo "$avg $min $max $p95 $jitter $loss_pct"
}

# ── Collect and sort ──
declare -a RESULT_LINES=()
for idx in $(seq 0 $((NODE_COUNT - 1))); do
    stats=$(calc_stats "$TMPDIR_BENCH/node_${idx}.csv")
    read -r avg min max p95 jitter loss_pct <<< "$stats"

    # Score: avg + loss*10 + jitter*2 (lower = better)
    if [[ "$avg" == "-" ]]; then
        score=999999
    else
        j=${jitter:-0}; [[ "$j" == "-" ]] && j=0
        score=$((avg + loss_pct * 10 + j * 2))
    fi

    RESULT_LINES+=("$score|${NODE_NAMES[$idx]}|$avg|$min|$max|$p95|$jitter|$loss_pct")
done

# Sort by score
IFS=$'\n' SORTED=($(printf '%s\n' "${RESULT_LINES[@]}" | sort -t'|' -k1 -n)); unset IFS

# ── Find max name width ──
max_name=4
for line in "${SORTED[@]}"; do
    IFS='|' read -r _ name _ <<< "$line"
    [[ ${#name} -gt $max_name ]] && max_name=${#name}
done

# ── Print table ──
fmt_ms() {
    if [[ "$1" == "-" ]]; then
        printf -- "${DIM}-${RESET}"
    else
        printf "%sms" "$1"
    fi
}

color_indicator() {
    local avg=$1 loss=$2
    if [[ "$avg" == "-" || $loss -ge 50 ]]; then
        printf "${RED}X${RESET}"
    elif [[ $loss -gt 0 || "$avg" -ge 500 ]]; then
        printf "${YELLOW}!${RESET}"
    elif [[ "$avg" -lt 200 ]]; then
        printf "${GREEN}*${RESET}"
    else
        printf " "
    fi
}

printf "  ${BOLD}%-${max_name}s  %7s  %7s  %7s  %7s  %7s  %5s${RESET}\n" \
    "Node" "Avg" "Min" "Max" "P95" "Jitter" "Loss"
printf "  %-${max_name}s  %7s  %7s  %7s  %7s  %7s  %5s\n" \
    "$(printf '%0.s-' $(seq 1 $max_name))" "-------" "-------" "-------" "-------" "-------" "-----"

for line in "${SORTED[@]}"; do
    IFS='|' read -r score name avg min max p95 jitter loss_pct <<< "$line"
    indicator=$(color_indicator "$avg" "$loss_pct")
    printf "%b %-${max_name}s  %7s  %7s  %7s  %7s  %7s  %4s%%\n" \
        "$indicator" "$name" \
        "$(fmt_ms "$avg")" "$(fmt_ms "$min")" "$(fmt_ms "$max")" \
        "$(fmt_ms "$p95")" "$(fmt_ms "$jitter")" "$loss_pct"
done

# ── Best node ──
echo ""
IFS='|' read -r _ best_name best_avg _ _ _ best_jitter best_loss <<< "${SORTED[0]}"
if [[ "$best_avg" != "-" ]]; then
    printf "  ${BOLD}${GREEN}Best: %s${RESET} (avg %sms, loss %s%%, jitter %s)\n" \
        "$best_name" "$best_avg" "$best_loss" \
        "$(if [[ "$best_jitter" == "-" ]]; then echo "-"; else echo "${best_jitter}ms"; fi)"
fi
