#!/usr/bin/env bash

search_tool_available() {
  command -v rg >/dev/null 2>&1
}

search_lines() {
  local pattern=$1
  shift

  if search_tool_available; then
    rg -n --no-heading "$pattern" "$@"
  else
    grep -REn -- "$pattern" "$@"
  fi
}

search_file_lines() {
  local pattern=$1
  local file=$2

  if search_tool_available; then
    rg -n "$pattern" "$file"
  else
    grep -En -- "$pattern" "$file"
  fi
}

search_file_quiet() {
  local pattern=$1
  local file=$2

  if search_tool_available; then
    rg -q "$pattern" "$file"
  else
    grep -Eq -- "$pattern" "$file"
  fi
}

search_file_exact_quiet() {
  local pattern=$1
  local file=$2

  if search_tool_available; then
    rg -qx "$pattern" "$file"
  else
    grep -Eqx -- "$pattern" "$file"
  fi
}
