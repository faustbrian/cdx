#!/usr/bin/env bash

set -euo pipefail

ROOT_DIRECTORY=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT_DIRECTORY"

./scripts/coverage.sh >/dev/null

ruby -rjson <<'RUBY'
summary = JSON.parse(File.read("target/coverage-summary.json"))
totals = summary.fetch("data").fetch(0).fetch("totals")

thresholds = {
  "lines" => 100.0,
  "functions" => 100.0,
  "regions" => 100.0,
}

failures = []
thresholds.each do |name, threshold|
  actual = totals.fetch(name).fetch("percent")
  next if actual >= threshold

  failures << "#{name} coverage #{format('%.2f', actual)}% is below #{format('%.2f', threshold)}%"
end

if failures.empty?
  puts "coverage audit passed"
  thresholds.each_key do |name|
    actual = totals.fetch(name).fetch("percent")
    puts "#{name} coverage: #{format('%.2f', actual)}%"
  end
  exit 0
end

warn failures.join("\n")
exit 1
RUBY
