// The shading models past `STANDARD` (ADR-0023 §6c, N2c), for the deferred lighting's extended
// permutation only (`MOBA_SHADING_MODELS`, defined by `deferred_light_extended.comp`). Included
// by lighting.glsl after its BRDF helpers; `shade()` calls `model_prepare` once (the
// light-invariant terms), `shade()` and `dynamic_lights()` call `model_direct` for each light,
// and `shade()` calls `model_ambient` once, so a `STANDARD` pixel in an extended tile shades
// exactly as in the standard permutation. Model data is in physical units
// (`decode_gbuffer`).
//
// - `CLEARCOAT`: one GGX coat lobe (IOR 1.5, F0 0.04, the base normal), the base attenuated by
//   the coat's Fresnel (`KHR_materials_clearcoat`).
// - `SHEEN`: the Charlie sheen lobe (Estevez and Kulla 2017) with Neubelt's visibility, the
//   base scaled by one minus the sheen's directional albedo, approximated by a fit to the
//   Charlie lobe's (`KHR_materials_sheen`).
// - `ANISOTROPIC`: anisotropic GGX with height-correlated Smith visibility (Heitz 2014),
//   alpha_t = mix(alpha, 1, strength²) along the tangent rotated by the stored angle in a
//   basis built from the normal (Duff et al. 2017; the G-buffer stage stores the mesh
//   tangent's rotated direction as its angle in this basis, so the basis's flip across
//   n.z = 0 does not show), and Filament's bent reflection vector for the ambient
//   (`KHR_materials_anisotropy`).
// - `THIN_FILM`: the Belcour-Barla thin-film Fresnel (`KHR_materials_iridescence`), the GLSL
//   twin of `moba_assets::thin_film_reflectance`, evaluated once per pixel at N.V: each
//   light's Schlick term is scaled by the film's reflectance over Schlick's there (ADR-0023
//   amendment 3). `model_f0_between`, `model_sensitivity` and `model_thin_film_fresnel` are
//   adapted from the Khronos glTF Sample Viewer's `iridescence.glsl` (Copyright The Khronos
//   Group Inc., Apache License 2.0; THIRD_PARTY.md), which implements Belcour and Barla 2017.
// - `FOLIAGE` and `TRANSMISSIVE_FALLBACK` shade as `STANDARD` here (the foliage's back
//   light is `deferred_light.comp`'s).
#ifndef MOBA_SHADING_MODELS_GLSL
#define MOBA_SHADING_MODELS_GLSL

#define MOBA_MODEL_CLEARCOAT 2u
#define MOBA_MODEL_SHEEN 3u
#define MOBA_MODEL_ANISOTROPIC 4u
#define MOBA_MODEL_THIN_FILM 5u

const float MOBA_COAT_F0 = 0.04; // IOR 1.5

// Duff et al. 2017: an orthonormal basis from a unit normal.
void model_basis(vec3 n, out vec3 t, out vec3 b) {
    float s = n.z >= 0.0 ? 1.0 : -1.0;
    float a = -1.0 / (s + n.z);
    float c = n.x * n.y * a;
    t = vec3(1.0 + s * n.x * n.x * a, s * c, -s * n.x);
    b = vec3(c, s + n.y * n.y * a, -n.y);
}

// The anisotropy's tangent and bitangent: the basis rotated by the stored angle.
void model_anisotropy_frame(Surface surface, vec3 n, out vec3 t, out vec3 b) {
    vec3 t0;
    vec3 b0;
    model_basis(n, t0, b0);
    float angle = surface.model_data.y;
    t = cos(angle) * t0 + sin(angle) * b0;
    b = cross(n, t);
}

float model_charlie_d(float alpha, float n_dot_h) {
    float inv = 1.0 / max(alpha, 1e-3);
    float sin2 = max(1.0 - n_dot_h * n_dot_h, 0.0078125);
    return (2.0 + inv) * pow(sin2, inv * 0.5) / (2.0 * MOBA_PI);
}

float model_neubelt_v(float n_dot_v, float n_dot_l) {
    return clamp(1.0 / (4.0 * (n_dot_l + n_dot_v - n_dot_l * n_dot_v)), 0.0, 1.0);
}

// The Charlie lobe's directional albedo, a fit over (N.V, roughness) to the white-sheen
// integral (within ~0.02 of it): high at grazing, larger with roughness.
float model_sheen_albedo(float n_dot_v, float roughness) {
    float grazing = pow(1.0 - n_dot_v, 4.0);
    return clamp(0.06 + 0.16 * roughness + 0.30 * grazing * (0.4 + roughness), 0.0, 1.0);
}

vec3 model_sheen_color(Surface surface) {
    return surface.model_data.x * mix(vec3(1.0), surface.albedo, surface.model_data.z);
}

float model_aniso_d(float at, float ab, float t_dot_h, float b_dot_h, float n_dot_h) {
    float a2 = at * ab;
    vec3 d = vec3(ab * t_dot_h, at * b_dot_h, a2 * n_dot_h);
    float d2 = dot(d, d);
    float w2 = a2 / max(d2, 1e-12);
    return a2 * w2 * w2 / MOBA_PI;
}

float model_aniso_v(float at, float ab, float t_dot_v, float b_dot_v, float t_dot_l, float b_dot_l,
                    float n_dot_v, float n_dot_l) {
    float lambda_v = n_dot_l * length(vec3(at * t_dot_v, ab * b_dot_v, n_dot_v));
    float lambda_l = n_dot_v * length(vec3(at * t_dot_l, ab * b_dot_l, n_dot_l));
    return 0.5 / max(lambda_v + lambda_l, 1e-5);
}

// --- Thin film (Belcour and Barla 2017; `moba_assets::thin_film_reflectance`) ------------
float model_f0_between(float transmitted, float incident) {
    float r = (transmitted - incident) / (transmitted + incident);
    return r * r;
}

vec3 model_sensitivity(float opd, vec3 shift) {
    float phase = 2.0 * MOBA_PI * opd * 1.0e-9;
    vec3 val = vec3(5.4856e-13, 4.4201e-13, 5.2481e-13);
    vec3 pos = vec3(1.6810e6, 1.7953e6, 2.2084e6);
    vec3 var = vec3(4.3278e9, 9.3046e9, 6.6121e9);
    vec3 xyz = val * sqrt(2.0 * MOBA_PI * var) * cos(pos * phase + shift)
        * exp(-phase * phase * var);
    xyz.x += 9.7470e-14 * sqrt(2.0 * MOBA_PI * 4.5282e9) * cos(2.2399e6 * phase + shift.x)
        * exp(-4.5282e9 * phase * phase);
    xyz /= 1.0685e-7;
    const mat3 xyz_to_rec709 = mat3(
        3.2404542, -0.9692660, 0.0556434,
        -1.5371385, 1.8760108, -0.2040259,
        -0.4985314, 0.0415560, 1.0572252);
    return xyz_to_rec709 * xyz;
}

vec3 model_thin_film_fresnel(float cos_theta, float film_ior, float thickness, vec3 base_f0) {
    float t = clamp(thickness / 0.03, 0.0, 1.0);
    float film = 1.0 + (film_ior - 1.0) * (t * t * (3.0 - 2.0 * t));
    float sin2 = (1.0 / (film * film)) * (1.0 - cos_theta * cos_theta);
    float cos2_sq = 1.0 - sin2;
    if (cos2_sq < 0.0) {
        return vec3(1.0);
    }
    float cos2 = sqrt(cos2_sq);
    float r0 = model_f0_between(film, 1.0);
    float r12 = r0 + (1.0 - r0) * pow(1.0 - clamp(cos_theta, 0.0, 1.0), 5.0);
    float t121 = 1.0 - r12;
    float phi12 = film < 1.0 ? MOBA_PI : 0.0;
    float phi21 = MOBA_PI - phi12;
    vec3 f0 = clamp(base_f0, vec3(0.0), vec3(0.9999));
    vec3 base_ior = (1.0 + sqrt(f0)) / (1.0 - sqrt(f0));
    vec3 r1 = (base_ior - film) / (base_ior + film);
    r1 *= r1;
    vec3 r23 = r1 + (1.0 - r1) * pow(1.0 - clamp(cos2, 0.0, 1.0), 5.0);
    vec3 phi23 = vec3(base_ior.r < film ? MOBA_PI : 0.0, base_ior.g < film ? MOBA_PI : 0.0,
                      base_ior.b < film ? MOBA_PI : 0.0);
    vec3 r123 = clamp(r12 * r23, vec3(1e-5), vec3(0.9999));
    vec3 rs = t121 * t121 * r23 / (1.0 - r123);
    vec3 result = r12 + rs;
    vec3 cm = rs - t121;
    vec3 root = sqrt(r123);
    vec3 phi = phi21 + phi23;
    float opd = 2.0 * film * thickness * cos2;
    for (int order = 1; order <= 2; order++) {
        float m = float(order);
        cm *= root;
        result += cm * 2.0 * model_sensitivity(m * opd, phi * m);
    }
    return max(result, vec3(0.0));
}

// The model's light-invariant terms, computed once per pixel by `model_prepare` (called by
// `shade()` before the sun and the light loop) and read by `model_direct` for each light.
struct ModelContext {
    // ANISOTROPIC: the tangent frame and the two GGX alphas, and V in that frame.
    vec3 t;
    vec3 b;
    float at;
    float ab;
    float t_dot_v;
    float b_dot_v;
    // SHEEN: the sheen colour, alpha and the base's scale under it. CLEARCOAT: the coat's
    // alpha (`sheen_alpha` reused).
    vec3 sheen_color;
    float sheen_alpha;
    float base_scale;
    // THIN_FILM: the film's reflectance over Schlick's, both at N.V (amendment 3: the film is
    // evaluated once per pixel, not per light), and the film's reflectance itself.
    vec3 film_ratio;
    vec3 film;
};
ModelContext moba_model;

void model_prepare(Surface surface, vec3 n, vec3 v, float n_dot_v, float alpha, vec3 f0) {
    uint model = surface.model;
    moba_model.film = f0;
    moba_model.film_ratio = vec3(1.0);
    if (model == MOBA_MODEL_CLEARCOAT) {
        float coat_roughness = clamp(surface.model_data.y, MOBA_MIN_ROUGHNESS, 1.0);
        moba_model.sheen_alpha = coat_roughness * coat_roughness;
    } else if (model == MOBA_MODEL_SHEEN) {
        float sheen_roughness = clamp(surface.model_data.y, 0.07, 1.0);
        vec3 color = model_sheen_color(surface);
        float peak = max(max(color.r, color.g), color.b);
        moba_model.sheen_color = color;
        moba_model.sheen_alpha = sheen_roughness * sheen_roughness;
        moba_model.base_scale = 1.0 - peak * model_sheen_albedo(n_dot_v, sheen_roughness);
    } else if (model == MOBA_MODEL_ANISOTROPIC) {
        model_anisotropy_frame(surface, n, moba_model.t, moba_model.b);
        float strength = surface.model_data.x;
        moba_model.at = mix(alpha, 1.0, strength * strength);
        moba_model.ab = alpha;
        moba_model.t_dot_v = dot(moba_model.t, v);
        moba_model.b_dot_v = dot(moba_model.b, v);
    } else if (model == MOBA_MODEL_THIN_FILM) {
        vec3 film = model_thin_film_fresnel(n_dot_v, surface.model_data.y, surface.model_data.x,
                                            f0);
        moba_model.film = film;
        moba_model.film_ratio = film / max(fresnel_schlick(f0, n_dot_v), vec3(1e-4));
    }
}

// One light's BRDF times N.L for the surface's model, from the `STANDARD` terms the caller
// computed (`diffuse`: kd x albedo x the wrapped N.L; `specular`: GGX x Smith x Schlick x
// N.L x the painted specular x the energy compensation) and `moba_model`.
vec3 model_direct(Surface surface, vec3 n, vec3 v, vec3 l, vec3 h, float n_dot_v,
                  float n_dot_l, float alpha, vec3 f0, vec3 diffuse, vec3 specular) {
    uint model = surface.model;
    if (model == MOBA_MODEL_CLEARCOAT) {
        float n_dot_h = max(dot(n, h), 0.0);
        float v_dot_h = max(dot(v, h), 0.0);
        float fc = (MOBA_COAT_F0 + (1.0 - MOBA_COAT_F0) * pow(1.0 - v_dot_h, 5.0))
            * surface.model_data.x;
        float coat_alpha = moba_model.sheen_alpha;
        float coat = MOBA_PI * distribution_ggx(n_dot_h, coat_alpha)
            * visibility_smith_ggx(n_dot_v, n_dot_l, coat_alpha) * fc * n_dot_l
            * surface.specular.w;
        return (diffuse + specular) * (1.0 - fc) + vec3(coat);
    }
    if (model == MOBA_MODEL_SHEEN) {
        float n_dot_h = max(dot(n, h), 0.0);
        vec3 sheen = MOBA_PI * moba_model.sheen_color
            * model_charlie_d(moba_model.sheen_alpha, n_dot_h)
            * model_neubelt_v(n_dot_v, n_dot_l) * n_dot_l;
        return (diffuse + specular) * moba_model.base_scale + sheen;
    }
    if (model == MOBA_MODEL_ANISOTROPIC) {
        float n_dot_h = max(dot(n, h), 0.0);
        float v_dot_h = max(dot(v, h), 0.0);
        float at = moba_model.at;
        float ab = moba_model.ab;
        float d = model_aniso_d(at, ab, dot(moba_model.t, h), dot(moba_model.b, h), n_dot_h);
        float vis = model_aniso_v(at, ab, moba_model.t_dot_v, moba_model.b_dot_v,
                                  dot(moba_model.t, l), dot(moba_model.b, l), n_dot_v, n_dot_l);
        vec3 fresnel = fresnel_schlick(f0, v_dot_h);
        return diffuse + MOBA_PI * d * vis * fresnel * n_dot_l * surface.specular.rgb
            * surface.specular.w * moba_energy_compensation;
    }
    if (model == MOBA_MODEL_THIN_FILM) {
        // The film's colour at N.V over Schlick's there, on each light's Schlick term.
        return diffuse + specular * moba_model.film_ratio;
    }
    return diffuse + specular;
}

// The ambient Fresnel term for the split sum: F0 for every model but the thin film, whose
// film reflectance at N.V (`model_prepare`'s) replaces it.
vec3 model_ambient_f0(Surface surface, vec3 f0) {
    return surface.model == MOBA_MODEL_THIN_FILM ? moba_model.film : f0;
}

// The specular ambient's lookup direction: Filament's bent reflection for anisotropy.
vec3 model_ambient_direction(Surface surface, vec3 n, vec3 v, vec3 r, float roughness) {
    if (surface.model != MOBA_MODEL_ANISOTROPIC) {
        return r;
    }
    vec3 b = moba_model.b;
    float strength = surface.model_data.x;
    vec3 anisotropic_tangent = cross(b, v);
    vec3 anisotropic_normal = cross(anisotropic_tangent, b);
    float bend = pow(1.0 - strength * (1.0 - roughness), 4.0);
    vec3 bent = normalize(mix(anisotropic_normal, n, bend));
    return reflect(-v, bent);
}

// The ambient after the base's: the coat or the sheen added, the base attenuated under them.
vec3 model_ambient(Surface surface, vec3 n, vec3 v, float n_dot_v, vec3 ambient_base,
                   float ao, float specular_ao) {
    if (surface.model == MOBA_MODEL_CLEARCOAT) {
        float coat_roughness = clamp(surface.model_data.y, MOBA_MIN_ROUGHNESS, 1.0);
        vec2 coat_brdf = brdf_lut_sample(n_dot_v, coat_roughness);
        float weight = surface.model_data.x;
        float fc = (MOBA_COAT_F0 + (1.0 - MOBA_COAT_F0) * pow(1.0 - n_dot_v, 5.0)) * weight;
        vec3 coat = ambient_specular(reflect(-v, n), coat_roughness)
            * (MOBA_COAT_F0 * coat_brdf.x + coat_brdf.y) * weight * surface.specular.w;
        return ambient_base * (1.0 - fc) + coat * specular_ao;
    }
    if (surface.model == MOBA_MODEL_SHEEN) {
        float sheen_roughness = clamp(surface.model_data.y, 0.07, 1.0);
        float albedo = model_sheen_albedo(n_dot_v, sheen_roughness);
        vec3 sheen = moba_model.sheen_color * albedo * ambient_irradiance(n) * ao;
        return ambient_base * moba_model.base_scale + sheen;
    }
    return ambient_base;
}

#endif
