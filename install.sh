#!/usr/bin/env sh
set -eu

repository='BrThiagoN/path-to-link'
os=$(uname -s)
arch=$(uname -m)

case "$os/$arch" in
    Darwin/arm64|Darwin/aarch64) target='aarch64-apple-darwin' ;;
    Darwin/x86_64|Darwin/amd64) target='x86_64-apple-darwin' ;;
    Linux/x86_64|Linux/amd64) target='x86_64-unknown-linux-gnu' ;;
    Linux/aarch64|Linux/arm64) target='aarch64-unknown-linux-gnu' ;;
    *) printf 'Unsupported platform: %s/%s\n' "$os" "$arch" >&2; exit 1 ;;
esac

asset_name="path-to-link-$target"
release_url="https://github.com/$repository/releases/latest/download"
install_dir="$HOME/.local/bin"
temp_dir=$(mktemp -d 2>/dev/null || mktemp -d -t path-to-link)
trap 'rm -rf "$temp_dir"' EXIT HUP INT TERM

download() {
    url=$1
    destination=$2
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$url" -o "$destination"
    elif command -v wget >/dev/null 2>&1; then
        wget -q "$url" -O "$destination"
    else
        printf 'Install curl or wget, then run this installer again.\n' >&2
        exit 1
    fi
}

download "$release_url/$asset_name" "$temp_dir/$asset_name"
download "$release_url/$asset_name.sha256" "$temp_dir/$asset_name.sha256"

expected=$(awk 'NR == 1 {print $1}' "$temp_dir/$asset_name.sha256")
if command -v sha256sum >/dev/null 2>&1; then
    actual=$(sha256sum "$temp_dir/$asset_name" | awk '{print $1}')
elif command -v shasum >/dev/null 2>&1; then
    actual=$(shasum -a 256 "$temp_dir/$asset_name" | awk '{print $1}')
else
    printf 'Could not find sha256sum or shasum to verify the download.\n' >&2
    exit 1
fi

if [ -z "$expected" ] || [ "$actual" != "$expected" ]; then
    printf 'The downloaded file failed its SHA-256 check.\n' >&2
    exit 1
fi

mkdir -p "$install_dir"
cp "$temp_dir/$asset_name" "$install_dir/path-to-link"
chmod 755 "$install_dir/path-to-link"

shell_path=$(printenv SHELL 2>/dev/null || printf 'sh')
shell_name=$(basename "$shell_path")
case "$shell_name" in
    fish)
        profile_file="$HOME/.config/fish/config.fish"
        mkdir -p "$(dirname "$profile_file")"
        if ! grep -Fq '.local/bin' "$profile_file" 2>/dev/null; then
            printf '\n# Path-to-Link\nset -gx PATH "$HOME/.local/bin" $PATH\n' >> "$profile_file"
        fi
        ;;
    zsh) profile_file="$HOME/.zprofile" ;;
    bash)
        if [ -f "$HOME/.bash_profile" ]; then
            profile_file="$HOME/.bash_profile"
        else
            profile_file="$HOME/.profile"
        fi
        ;;
    *) profile_file="$HOME/.profile" ;;
esac

if [ "$shell_name" != 'fish' ] && ! grep -Fq '.local/bin' "$profile_file" 2>/dev/null; then
    printf '\n# Path-to-Link\nexport PATH="$HOME/.local/bin:$PATH"\n' >> "$profile_file"
fi

printf 'Installed Path-to-Link to %s/path-to-link\n' "$install_dir"
printf 'Added ~/.local/bin to %s if it was not already present.\n' "$profile_file"
printf 'Open a new terminal, then run: path-to-link --help\n'
