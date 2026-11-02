#!/usr/bin/env python3
"""Write the synthetic capability fixtures (ADR-0020 §2b) from the recorded RTX dump.

Each one stands for a device we do not own, labelled "synthetic", built from public facts
about that device class: what matters for admission is which extensions, features and limits
it lacks. usage: scripts/make-synthetic-fixtures.py crates/render/src/caps/fixtures
"""
import copy, json, sys
from pathlib import Path

out = Path(sys.argv[1])
rtx = json.loads((out / "rtx-5070-ti.json").read_text())

RT_EXT = ["VK_KHR_acceleration_structure", "VK_KHR_deferred_host_operations", "VK_KHR_ray_query",
          "VK_KHR_ray_tracing_pipeline", "VK_KHR_ray_tracing_maintenance1",
          "VK_KHR_ray_tracing_position_fetch", "VK_EXT_ray_tracing_invocation_reorder",
          "VK_KHR_opacity_micromap", "VK_EXT_opacity_micromap"]
RT_FEAT = ["accelerationStructure", "rayQuery", "rayTracingPipeline", "rayTracingMaintenance1",
           "rayTracingPositionFetch", "rayTracingInvocationReorder"]
NV_ONLY = [e for e in rtx["extensions"] if e.startswith("VK_NV") or e.startswith("VK_NVX")]
V14_EXT = ["VK_KHR_push_descriptor", "VK_KHR_maintenance5", "VK_KHR_maintenance6",
           "VK_KHR_dynamic_rendering_local_read", "VK_KHR_index_type_uint8",
           "VK_EXT_index_type_uint8", "VK_EXT_host_image_copy"]
FSR_FEAT = ["pipelineFragmentShadingRate", "primitiveFragmentShadingRate",
            "attachmentFragmentShadingRate"]
V14 = ["pushDescriptor", "maintenance5", "maintenance6", "dynamicRenderingLocalRead",
       "indexTypeUint8", "hostImageCopy"]


def derive(name, vendor, device, kind, api, driver_id, driver_name, driver_info,
           drop_ext=(), add_ext=(), drop_feat=(), features=None, limits=None, v14=True):
    caps = copy.deepcopy(rtx)
    caps.update(source="synthetic", name=name, vendor_id=vendor, device_id=device, kind=kind,
                api_version=api, driver_id=driver_id, driver_name=driver_name,
                driver_info=driver_info)
    caps["extensions"] = sorted((set(caps["extensions"]) - set(drop_ext)) | set(add_ext))
    for feature in drop_feat:
        caps["features"].pop(feature, None)
    if not v14:
        caps["extensions"] = [e for e in caps["extensions"] if e not in V14_EXT]
        for feature in V14:
            caps["features"].pop(feature, None)
    # The probe records an extension's feature bits only when it is advertised.
    if "VK_KHR_fragment_shading_rate" not in caps["extensions"]:
        for feature in FSR_FEAT:
            caps["features"].pop(feature, None)
    caps["features"].update(features or {})
    caps["limits"].update(limits or {})
    return caps


fixtures = {
    # Apple M1 through MoltenVK: the portability subset, no RT or mesh shaders (MoltenVK
    # PRs #2771/#2842 are unmerged WIP), 1.3 at most.
    "moltenvk-m1": derive(
        "Apple M1 (synthetic, MoltenVK)", 0x106B, 0, "integrated", "1.3.296", 14, "MoltenVK",
        "1.4.2", drop_ext=RT_EXT + NV_ONLY + ["VK_EXT_mesh_shader", "VK_KHR_fragment_shading_rate"],
        add_ext=["VK_KHR_portability_subset"], drop_feat=RT_FEAT + ["meshShader", "taskShader"],
        v14=False, limits={"maxPerStageDescriptorSamplers": 16}),
    # Apple M1 through Mesa KosmicKrisp: conformant, so no portability subset; its driver ID
    # (VK_DRIVER_ID_MESA_KOSMICKRISP = 28) is what marks it.
    "kosmickrisp-m1": derive(
        "Apple M1 (synthetic, KosmicKrisp)", 0x106B, 0, "integrated", "1.3.335", 28,
        "KosmicKrisp", "Mesa (synthetic)",
        drop_ext=RT_EXT + NV_ONLY + ["VK_EXT_mesh_shader", "VK_KHR_fragment_shading_rate"],
        drop_feat=RT_FEAT + ["meshShader", "taskShader"], v14=False),
    # NVK (VK_DRIVER_ID_MESA_NVK = 24) on Turing: mesh shaders and fragment shading rate, no RT.
    "nvk-rtx-2060": derive(
        "NVIDIA GeForce RTX 2060 (synthetic, NVK)", 0x10DE, 0x1F08, "discrete", "1.4.335", 24,
        "NVK", "Mesa (synthetic)", drop_ext=RT_EXT + NV_ONLY, drop_feat=RT_FEAT),
    # Pascal: no hardware RT, so no ray query or acceleration structures in Vulkan.
    "gtx-1060": derive(
        "NVIDIA GeForce GTX 1060 6GB (synthetic)", 0x10DE, 0x1C03, "discrete", "1.4.312", 4,
        "NVIDIA", "580 (synthetic)",
        drop_ext=RT_EXT + ["VK_EXT_mesh_shader", "VK_KHR_fragment_shading_rate"]
        + [e for e in NV_ONLY if e != "VK_NV_low_latency2"],
        drop_feat=RT_FEAT + ["meshShader", "taskShader"]),
    # Intel UHD 620 (Gen9.5) on ANV (VK_DRIVER_ID_INTEL_OPEN_SOURCE_MESA = 6): no RT, no mesh.
    "uhd-620": derive(
        "Intel(R) UHD Graphics 620 (synthetic)", 0x8086, 0x5917, "integrated", "1.4.335", 6,
        "Intel open-source Mesa driver", "Mesa (synthetic)",
        drop_ext=RT_EXT + NV_ONLY + ["VK_EXT_mesh_shader", "VK_KHR_fragment_shading_rate"],
        drop_feat=RT_FEAT + ["meshShader", "taskShader"]),
    # A device with RT but without mesh shaders: mesh shaders are optional (§1e), so Tiered.
    "rt-no-mesh": derive(
        "Synthetic RT GPU without mesh shaders", 0x10DE, 0, "discrete", rtx["api_version"],
        rtx["driver_id"], rtx["driver_name"], "synthetic",
        drop_ext=["VK_EXT_mesh_shader"], drop_feat=["meshShader", "taskShader"]),
}
for name, caps in fixtures.items():
    (out / f"{name}.json").write_text(json.dumps(caps, indent=2, sort_keys=False) + "\n")
    print(name, len(caps["extensions"]), "extensions")
