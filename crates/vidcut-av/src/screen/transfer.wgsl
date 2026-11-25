fn decode(c_in: vec3<f32>, transfer: u32, scale: f32) -> vec3<f32> {
    var c = max(c_in, vec3<f32>(0.0));
    if transfer == 1u {
        let e = pow(c, vec3<f32>(1.0 / 78.84375));
        let num = max(e - vec3<f32>(0.8359375), vec3<f32>(0.0));
        let den = max(vec3<f32>(18.8515625) - 18.6875 * e, vec3<f32>(1.17549435e-38));
        c = pow(num / den, vec3<f32>(1.0 / 0.1593017578125));
    } else {
        let high = (exp((c - vec3<f32>(0.55991073)) / 0.17883277) + vec3<f32>(0.28466892)) / 12.0;
        c = select(high, c * c / 3.0, c <= vec3<f32>(0.5));
        c = pow(c, vec3<f32>(1.2));
    }
    return c * scale;
}
