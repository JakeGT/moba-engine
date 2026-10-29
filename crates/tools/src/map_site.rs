//! The simulation map facts the terrain generator and the map-visuals consistency check read
//! (ADR-0014), built from a content pack's map. Shared by `content-check` and its tests.

/// `map`'s extent, lanes, walls, structures and spawns as a [`moba_assets::TerrainSite`].
pub(crate) fn terrain_site(map: &moba_game::MapSpec) -> moba_assets::TerrainSite {
    let point = |p: moba_core::FixVec2| [p.x.to_f32(), p.y.to_f32()];
    let mut site = moba_assets::TerrainSite::new(map.extent.to_f32());
    for lane in &map.lanes {
        site = site.with_lane(lane.path.iter().copied().map(point).collect());
    }
    for wall in &map.walls {
        site = site.with_wall(point(wall.a), point(wall.b));
    }
    for structure in &map.structures {
        site = site.with_structure(point(structure.position));
    }
    site.with_spawn(point(map.blue_spawn))
        .with_spawn(point(map.red_spawn))
}
