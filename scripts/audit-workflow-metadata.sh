#!/usr/bin/env bash

set -euo pipefail

readonly workflow_root="${1:-.github/workflows}"

if [[ ! -d "${workflow_root}" ]]; then
  printf 'workflow metadata audit root is not a directory: %s\n' \
    "${workflow_root}" >&2
  exit 1
fi

ruby - "${workflow_root}" <<'RUBY'
require "pathname"
require "psych"

workflow_root = Pathname(ARGV.fetch(0))
workflow_paths = workflow_root.glob("*.yml").sort

if workflow_paths.empty?
  warn "workflow metadata audit found no workflow files in: #{workflow_root}"
  exit 1
end

failures = []

workflow_paths.each do |workflow_path|
  document = Psych.safe_load(
    File.read(workflow_path),
    permitted_classes: [],
    aliases: false
  ) || {}
  jobs = document["jobs"]

  unless document.key?("permissions")
    failures << "#{workflow_path}: workflow must declare top-level permissions"
  end

  unless jobs.is_a?(Hash) && !jobs.empty?
    failures << "#{workflow_path}: workflow must declare at least one job"
    next
  end

  jobs.each do |job_name, job|
    unless job.is_a?(Hash)
      failures << "#{workflow_path}: job #{job_name} must be a mapping"
      next
    end

    timeout = job["timeout-minutes"]
    unless timeout.is_a?(Integer) && timeout.positive?
      failures << "#{workflow_path}: job #{job_name} must declare a positive timeout-minutes"
    end
  end
end

if failures.empty?
  puts "workflow metadata audit passed"
  exit 0
end

warn failures.join("\n")
exit 1
RUBY
