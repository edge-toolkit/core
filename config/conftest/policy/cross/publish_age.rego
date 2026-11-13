# The cargo and mise dependency cooldowns must name the same age, and neither may be switched off.
# mise writes the age as `7d` and cargo as `7 days`, so the two match when their numbers are equal and their units
# start with the same letter. Cargo's `months` is not accepted: its `m` would pair with mise's minutes.
package cross

import data.paths

mise_age_pattern := `^([0-9]+)([smhdw])$`

cargo_age_pattern := `^([0-9]+) (second|minute|hour|day|week)s?$`

cargo_config := file.contents if {
	some file in paths.files
	file.path == ".cargo/config.toml"
}

mise_config := file.contents if {
	some file in paths.files
	file.path == ".mise/config.toml"
}

cargo_age := cargo_config.registry["global-min-publish-age"]

mise_age := mise_config.settings.minimum_release_age

# An age as [number, first letter of its unit], undefined when the value is not in the tool's accepted form.
age_parts(value, pattern) := [to_number(match[1]), substring(match[2], 0, 1)] if {
	is_string(value)
	some match in regex.find_all_string_submatch_n(pattern, value, 1)
}

cargo_parts := age_parts(cargo_age, cargo_age_pattern)

mise_parts := age_parts(mise_age, mise_age_pattern)

# Every cargo age that outranks the global one, as [where it is set, value].
cargo_override contains ["[registry] min-publish-age", age] if {
	age := cargo_config.registry["min-publish-age"]
}

cargo_override contains [where, age] if {
	some name, registry in cargo_config.registries
	age := registry["min-publish-age"]
	where := $"[registries.{name}] min-publish-age"
}

matches_mise(age) if age_parts(age, cargo_age_pattern) == mise_parts

zero_age if {
	some parts in [cargo_parts, mise_parts]
	parts[0] == 0
}

# Cargo's publish age lives in .cargo/config.toml, so the file must exist.
deny contains "publish age: .cargo/config.toml is missing; it sets cargo's [registry] global-min-publish-age" if {
	not cargo_config
}

# .cargo/config.toml must set cargo's publish age.
deny contains "publish age: .cargo/config.toml sets no [registry] global-min-publish-age" if {
	cargo_config
	not cargo_age
}

# .mise/config.toml must set mise's release age.
deny contains "publish age: .mise/config.toml sets no [settings] minimum_release_age" if {
	mise_config
	not mise_age
}

# Cargo's publish age must be a number and a unit word, so it can be compared with mise's.
deny contains msg if {
	cargo_age
	not cargo_parts
	msg := $"publish age: cargo global-min-publish-age \"{cargo_age}\" is not `<n> <seconds|minutes|hours|days|weeks>`"
}

# mise's release age must be a number and a unit letter, so it can be compared with cargo's.
deny contains msg if {
	mise_age
	not mise_parts
	msg := $"publish age: mise minimum_release_age \"{mise_age}\" is not `<n><s|m|h|d|w>`"
}

# Neither age may be zero, which turns the cooldown off.
deny contains msg if {
	zero_age
	msg := $"publish age: cargo \"{cargo_age}\" / mise \"{mise_age}\" must not be zero, which turns the cooldown off"
}

# Cargo and mise must hold dependencies back by the same age.
deny contains msg if {
	cargo_parts != mise_parts
	msg := $"publish age: cargo global-min-publish-age \"{cargo_age}\" != mise minimum_release_age \"{mise_age}\""
}

# A registry-specific cargo age replaces the global one for that registry, so it must also match mise's.
deny contains msg if {
	some override in cargo_override
	[where, age] := override
	not matches_mise(age)
	msg := $"publish age: cargo {where} \"{age}\" != mise minimum_release_age \"{mise_age}\""
}

# The resolver override that lets too-new versions through is for one-off runs, never committed.
deny contains "publish age: .cargo/config.toml must not set [resolver] incompatible-publish-age = \"allow\"" if {
	cargo_config.resolver["incompatible-publish-age"] == "allow"
}
