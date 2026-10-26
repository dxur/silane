fn main() {
    // Compiles every hw/*.si file to $OUT_DIR/silane/<stem>.rs.
    silane_lang::build::compile_dir("hw");
}
