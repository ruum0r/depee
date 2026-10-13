#!/usr/bin/env bash
# Fails if any package installed in these fixtures was published less than
# MIN_DAYS (default 3) days ago. Bun 1.3 and Yarn Classic cannot enforce a
# release-age gate themselves, so run this after every install.
set -euo pipefail
cd "$(dirname "$0")"
min_days=${MIN_DAYS:-3}
cutoff=$(node -e 'console.log(new Date(Date.now() - process.argv[1] * 864e5).toISOString())' "$min_days")
status=0
specs=$(find . -path '*/node_modules/*' -name package.json -print0 |
  xargs -0 node -e '
    const seen = new Set();
    for (const file of process.argv.slice(1)) {
      try {
        const { name, version } = require(require("path").resolve(file));
        if (name && version && !name.startsWith("@fx/")) seen.add(`${name}@${version}`);
      } catch {}
    }
    console.log([...seen].sort().join("\n"));
  ')
while read -r spec; do
  [ -z "$spec" ] && continue
  name=${spec%@*} version=${spec##*@}
  published=$(npm view "$name" "time[$version]")
  if [[ -z "$published" || ! "$published" < "$cutoff" ]]; then
    echo "TOO NEW: $spec ($published)"
    status=1
  fi
done <<< "$specs"
[ "$status" -eq 0 ] && echo "All installed packages are older than $min_days days."
exit "$status"
