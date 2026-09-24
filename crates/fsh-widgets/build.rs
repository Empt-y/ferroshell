fn main() {
    // `include_dir!` embeds ../../assets; make sure edits there trigger a rebuild.
    println!("cargo:rerun-if-changed=../../assets");
}
