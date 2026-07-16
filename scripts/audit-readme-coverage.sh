#!/usr/bin/env bash

set -euo pipefail

ROOT_DIRECTORY=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT_DIRECTORY"

if [[ ! -f target/coverage-summary.json ]]; then
  ./scripts/coverage.sh >/dev/null
fi

ruby -rjson <<'RUBY'
readme = File.read("README.md")
summary = JSON.parse(File.read("target/coverage-summary.json"))
totals = summary.fetch("data").fetch(0).fetch("totals")

expected = {
  "lines" => format("%.2f", totals.fetch("lines").fetch("percent")),
  "functions" => format("%.2f", totals.fetch("functions").fetch("percent")),
  "regions" => format("%.2f", totals.fetch("regions").fetch("percent")),
}

missing = expected.each_with_object([]) do |(name, percent), memo|
  needle = "- #{name}: `#{percent}%`"
  memo << needle unless readme.include?(needle)
end

if missing.empty?
  puts "readme coverage audit passed"
  expected.each do |name, percent|
    puts "#{name} coverage documented at #{percent}%"
  end
  exit 0
end

warn "README coverage snapshot is stale or missing"
missing.each do |needle|
  warn "missing README line: #{needle}"
end
exit 1
RUBY
