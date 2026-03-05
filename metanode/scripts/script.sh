#!/bin/bash
# Detect desktop environment
DESKTOP=${XDG_CURRENT_DESKTOP:-""}
IDE_NAME="Antigravity"
IDE_NAME_LOWER=$(echo "$IDE_NAME" | tr '[:upper:]' '[:lower:]')

# Function to modify .desktop file
modify_desktop_file() {
local desktop_file="$1"
local backup_file="${desktop_file}.bak"

# Create backup
cp "$desktop_file" "$backup_file"

# Check if flag already exists
if grep -q "--remote-debugging-port=9000" "$desktop_file"; then
echo "Flag already present in $desktop_file"
return 0
fi

# Add flag to Exec line
sed -i 's|^Exec=.*|& --remote-debugging-port=9000|' "$desktop_file"

# Also add flag to TryExec if present
if grep -q "^TryExec=" "$desktop_file"; then
sed -i 's|^TryExec=.*|& --remote-debugging-port=9000|' "$desktop_file"
fi

echo "Modified $desktop_file"
return 0
}

# Search for .desktop files in common locations
DESKTOP_DIRS=(
"$HOME/.local/share/applications"
"/usr/share/applications"
"/usr/local/share/applications"
)

for dir in "${DESKTOP_DIRS[@]}"; do
if [ -d "$dir" ]; then
for file in "$dir"/*.desktop; do
if [ -f "$file" ]; then
if grep -qi "$IDE_NAME_LOWER" "$file" || grep -qi "cursor" "$file"; then
echo "Found: $file"
modify_desktop_file "$file"
fi
fi
done
fi
done

echo "Script completed. Please close and restart Antigravity."