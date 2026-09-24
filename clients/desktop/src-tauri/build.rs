fn main() {
    // Sign-in configuration is compiled in (see src/account.rs).
    println!("cargo:rerun-if-env-changed=ZWIIM_SUPABASE_URL");
    println!("cargo:rerun-if-env-changed=ZWIIM_SUPABASE_ANON_KEY");
    tauri_build::build()
}
