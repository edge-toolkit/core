from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="BuildInfo")


@_attrs_define
class BuildInfo:
    """How the answering server was built.

    Returned by `GET /info`, so an operator can tell which release, toolchain and feature set a hub is running.

        Attributes:
            features (list[str]): The crate features it was built with, which name the services it can mount.
            name (str): The crate the server was built from.
            profile (str): The cargo profile, `debug` or `release`.
            rustc (str): The `rustc --version` that compiled it.
            target (str): The target triple it was compiled for.
            version (str): Its version.
    """

    features: list[str]
    name: str
    profile: str
    rustc: str
    target: str
    version: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        features = self.features

        name = self.name

        profile = self.profile

        rustc = self.rustc

        target = self.target

        version = self.version

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "features": features,
                "name": name,
                "profile": profile,
                "rustc": rustc,
                "target": target,
                "version": version,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        features = cast(list[str], d.pop("features"))

        name = d.pop("name")

        profile = d.pop("profile")

        rustc = d.pop("rustc")

        target = d.pop("target")

        version = d.pop("version")

        build_info = cls(
            features=features,
            name=name,
            profile=profile,
            rustc=rustc,
            target=target,
            version=version,
        )

        build_info.additional_properties = d
        return build_info

    @property
    def additional_keys(self) -> list[str]:
        return list(self.additional_properties.keys())

    def __getitem__(self, key: str) -> Any:
        return self.additional_properties[key]

    def __setitem__(self, key: str, value: Any) -> None:
        self.additional_properties[key] = value

    def __delitem__(self, key: str) -> None:
        del self.additional_properties[key]

    def __contains__(self, key: str) -> bool:
        return key in self.additional_properties
