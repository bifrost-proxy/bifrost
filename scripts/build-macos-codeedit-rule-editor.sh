#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MACOS_DIR="$ROOT_DIR/apps/macos"
CODEEDIT_SCRATCH_PATH="${BIFROST_CODEEDIT_SCRATCH_PATH:-$MACOS_DIR/.build-codeedit}"
CHECKOUTS_DIR="$CODEEDIT_SCRATCH_PATH/checkouts"

export BIFROST_BUILD_CODEEDIT_RULE_EDITOR=1

swift package --package-path "$MACOS_DIR" --scratch-path "$CODEEDIT_SCRATCH_PATH" resolve

patch_swiftlint_plugin() {
  local package_file="$1"
  [[ -f "$package_file" ]] || return 0
  chmod u+w "$package_file" || true
  ruby -0pi -e '
    gsub(/,?\n\s*\/\/ SwiftLint\n\s*\.package\(\n\s*url: "https:\/\/github\.com\/lukepistrol\/SwiftLintPlugin",\n\s*(?:from: "[^"]+"|\.upToNextMajor\(from: "[^"]+"\))\n\s*\)/, "")
    gsub(/,?\n\s*plugins: \[\n\s*\.plugin\(name: "SwiftLint", package: "SwiftLintPlugin"\)\n\s*\]/, "")
  ' "$package_file"
}

patch_codeedit_symbols_resources() {
  local package_file="$CHECKOUTS_DIR/CodeEditSymbols/Package.swift"
  [[ -f "$package_file" ]] || return 0
  chmod u+w "$package_file" || true
  ruby - "$package_file" <<'RUBY'
path = ARGV.fetch(0)
text = File.read(path)
patched = text.gsub(
  /\.target\(\s*name: "CodeEditSymbols",\s*dependencies: \[\]\s*\)/m,
  ".target(\n            name: \"CodeEditSymbols\",\n            dependencies: [],\n            resources: [.process(\"Symbols.xcassets\")]\n        )"
)
File.write(path, patched) if patched != text
RUBY
}

patch_codeedit_previews() {
  local panel_dir="$CHECKOUTS_DIR/CodeEditSourceEditor/Sources/CodeEditSourceEditor/Find/PanelView"
  [[ -d "$panel_dir" ]] || return 0
  chmod -R u+w "$panel_dir" || true
  ruby - "$panel_dir" <<'RUBY'
dir = ARGV.fetch(0)
Dir["#{dir}/*.swift"].each do |path|
  text = File.read(path)
  out = String.new
  index = 0
  while (start = text.index(/^#Preview.*?\{\n/m, index))
    out << text[index...start]
    brace_start = text.index("{", start)
    depth = 0
    pos = brace_start
    in_string = false
    escaped = false
    while pos < text.length
      ch = text[pos]
      if in_string
        if escaped
          escaped = false
        elsif ch == "\\"
          escaped = true
        elsif ch == "\""
          in_string = false
        end
      else
        if ch == "\""
          in_string = true
        elsif ch == "{"
          depth += 1
        elsif ch == "}"
          depth -= 1
          if depth == 0
            pos += 1
            pos += 1 while pos < text.length && text[pos] == "\n"
            break
          end
        end
      end
      pos += 1
    end
    index = pos
  end
  out << text[index..]
  File.write(path, out) if out != text
end
RUBY
}

patch_codeedit_color_space() {
  local source_dir="$CHECKOUTS_DIR/CodeEditSourceEditor/Sources/CodeEditSourceEditor"
  [[ -d "$source_dir" ]] || return 0
  chmod -R u+w "$source_dir/Minimap" "$source_dir/ReformattingGuide" 2>/dev/null || true
  ruby - "$source_dir" <<'RUBY'
dir = ARGV.fetch(0)
{
  "#{dir}/Minimap/MinimapView.swift" => "let isLightMode = (theme.background.usingColorSpace(.deviceRGB)?.brightnessComponent ?? 0.0) > 0.5",
  "#{dir}/ReformattingGuide/ReformattingGuideView.swift" => "let isLightMode = (theme.background.usingColorSpace(.deviceRGB)?.brightnessComponent ?? 0.0) > 0.5"
}.each do |path, replacement|
  next unless File.file?(path)
  text = File.read(path)
  patched = text.gsub("let isLightMode = theme.background.brightnessComponent > 0.5", replacement)
  File.write(path, patched) if patched != text
end
RUBY
}

create_dev_app_bundle() {
  local bin_dir
  bin_dir="$(swift build --package-path "$MACOS_DIR" --scratch-path "$CODEEDIT_SCRATCH_PATH" --skip-update --show-bin-path | tail -n 1)"

  local executable="$bin_dir/Bifrost"
  local resource_bundle="$bin_dir/Bifrost_Bifrost.bundle"
  local app_dir="$CODEEDIT_SCRATCH_PATH/Bifrost-CodeEdit.app"
  local contents_dir="$app_dir/Contents"
  local bundle_version="${BIFROST_VERSION:-0.0.129}"

  if [[ ! -x "$executable" ]]; then
    echo "missing built Bifrost executable: $executable" >&2
    exit 1
  fi

  rm -rf "$app_dir"
  mkdir -p "$contents_dir/MacOS" "$contents_dir/Resources"
  install -m 755 "$executable" "$contents_dir/MacOS/Bifrost"
  install -m 644 "$ROOT_DIR/assets/bifrost.icns" "$contents_dir/Resources/bifrost.icns"

  if [[ -d "$resource_bundle" ]]; then
    cp -R "$resource_bundle" "$app_dir/Bifrost_Bifrost.bundle"
    cp -R "$resource_bundle" "$contents_dir/Resources/Bifrost_Bifrost.bundle"
  fi

  cat >"$contents_dir/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleDisplayName</key>
  <string>Bifrost CodeEdit</string>
  <key>CFBundleExecutable</key>
  <string>Bifrost</string>
  <key>CFBundleIconFile</key>
  <string>bifrost</string>
  <key>CFBundleIdentifier</key>
  <string>com.bifrost.native.mac.codeedit</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>Bifrost CodeEdit</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>${bundle_version}</string>
  <key>CFBundleVersion</key>
  <string>${bundle_version}</string>
  <key>LSApplicationCategoryType</key>
  <string>public.app-category.developer-tools</string>
  <key>LSMinimumSystemVersion</key>
  <string>13.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
  <key>NSPrincipalClass</key>
  <string>NSApplication</string>
</dict>
</plist>
PLIST

  echo "$app_dir"
}

patch_swiftlint_plugin "$CHECKOUTS_DIR/CodeEditTextView/Package.swift"
patch_swiftlint_plugin "$CHECKOUTS_DIR/CodeEditSourceEditor/Package.swift"
patch_codeedit_symbols_resources
patch_codeedit_previews
patch_codeedit_color_space

swift build --package-path "$MACOS_DIR" --scratch-path "$CODEEDIT_SCRATCH_PATH" --skip-update "$@"
create_dev_app_bundle
