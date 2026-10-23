#!/usr/bin/env ruby
# frozen_string_literal: true

# Disables upstream workflows that only make sense in ente-io/ente: release
# builds, deploys, translation syncs and other push or schedule triggers that
# need Ente's secrets. Lint and test workflows (pull_request CI and the
# workflow_call jobs it runs) stay on, as do the fork's own fork-*.yml.
#
# Disabling goes through the GitHub API, so no upstream file is edited and
# upstream merges stay conflict-free. New upstream workflows are caught on
# the next weekly run.
#
# Usage: quiet-upstream-workflows.rb [--dry-run]
# Needs GH_TOKEN (actions: write) and GITHUB_REPOSITORY unless --dry-run.

require "open3"
require "yaml"

KEEP = %w[ci.yml change-approval.yml].freeze
NOISY_TRIGGERS = %w[push schedule pull_request_target release].freeze

def triggers(path)
  workflow = YAML.safe_load(File.read(path), aliases: true) || {}
  events = workflow["on"] || workflow[true]
  case events
  when String then [events]
  when Array then events.grep(String)
  when Hash then events.keys.map(&:to_s)
  else []
  end
end

def gh(*args)
  out, status = Open3.capture2e("gh", *args)
  [out.strip, status.success?]
end

dry_run = ARGV.include?("--dry-run")
repo = ENV.fetch("GITHUB_REPOSITORY", nil)
abort("GITHUB_REPOSITORY is not set") if repo.nil? && !dry_run

root = File.expand_path("../..", __dir__)
paths = Dir[File.join(root, ".github/workflows/*.{yml,yaml}")].sort
failed = false

paths.each do |path|
  file = File.basename(path)
  next if file.start_with?("fork-") || KEEP.include?(file)
  next if (triggers(path) & NOISY_TRIGGERS).empty?

  if dry_run
    puts "would disable #{file}"
    next
  end

  state, ok = gh("api", "repos/#{repo}/actions/workflows/#{file}", "--jq", ".state")
  unless ok
    # Not registered yet (file not on the default branch); next run gets it.
    puts "skip #{file}: #{state}"
    next
  end
  if state.start_with?("disabled")
    puts "already disabled #{file}"
    next
  end

  out, ok = gh("workflow", "disable", file, "--repo", repo)
  if ok
    puts "disabled #{file}"
  else
    warn "failed to disable #{file}: #{out}"
    failed = true
  end
end

exit(failed ? 1 : 0)
