#!/bin/bash
# Credits: claude LMAO
set -e

# Function to display usage information
usage() {
    echo "Usage: $0 <binary_file> <version> <firmware> <hardware> <build_time>"
    echo "  <binary_file>: Path to the .bin file"
    echo "  <version>: Version string (max 32 chars)"
    echo "  <firmware>: Firmware string (max 16 chars)"
    echo "  <hardware>: Hardware string (max 16 chars)"
    echo "  <build_time>: Build timestamp (unsigned 64-bit number)"
    exit 1
}

# Function to validate string length
validate_string() {
    local str="$1"
    local max_len="$2"
    local field_name="$3"
    
    if [ ${#str} -gt $max_len ]; then
        echo "Error: $field_name exceeds maximum length of $max_len characters"
        exit 1
    fi
}

# Function to pad string with zeros
pad_string() {
    local str="$1"
    local max_len="$2"
    printf "%-${max_len}s" "$str" | tr ' ' '\0'
}

# Check if all arguments are provided
if [ $# -ne 5 ]; then
    usage
fi

binary_file="$1"
version="$2"
firmware="$3"
hardware="$4"
build_time="$5"

# Check if file exists and has .bin extension
if [ ! -f "$binary_file" ]; then
    echo "Error: File '$binary_file' does not exist"
    exit 1
fi

if [[ "$binary_file" != *.bin ]]; then
    echo "Error: File must have .bin extension"
    exit 1
fi

# Validate input lengths
validate_string "$version" 32 "Version"
validate_string "$firmware" 16 "Firmware"
validate_string "$hardware" 16 "Hardware"

# Create temporary file
temp_file=$(mktemp)

# Pad strings and write to temp file
pad_string "$version" 32 > "$temp_file"
pad_string "$firmware" 16 >> "$temp_file"
pad_string "$hardware" 16 >> "$temp_file"

# Convert build_time to 8 bytes and append
printf "%016x" "$build_time" | xxd -r -p >> "$temp_file"

# Append temp file to binary file
if cat "$temp_file" >> "$binary_file"; then
    echo "Successfully appended metadata to '$binary_file'"
    echo "  Version: $version"
    echo "  Firmware: $firmware"
    echo "  Hardware: $hardware"
    echo "  Build Time: $build_time"
else
    echo "Error: Failed to append metadata"
    rm "$temp_file"
    exit 1
fi

# Clean up
rm "$temp_file"
