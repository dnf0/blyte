//! CI matrix expansion (`${{ matrix.<key> }}`) and `make -C <dir> <target>` check recognition.

use blyte_cli::init::detect_ci;

fn run_detect(workflow: &str) -> Vec<String> {
    let d = tempfile::tempdir().unwrap();
    let wf = d.path().join(".github/workflows/ci.yml");
    std::fs::create_dir_all(wf.parent().unwrap()).unwrap();
    std::fs::write(&wf, workflow).unwrap();
    detect_ci(d.path())
}

#[test]
fn matrix_make_step_expands_per_service() {
    let wf = "\
jobs:
  test-unit:
    strategy:
      fail-fast: false
      matrix:
        service: [api-service, shared-utils, portfolio-splitter, raster-reader, premise-reader]
    steps:
      - uses: actions/checkout@v4
      - name: Install system deps
        run: |
          sudo apt-get update
          sudo apt-get install -y gdal-bin
      - name: Sync environment
        run: |
          uv sync
      - name: Run tests with coverage
        run: |
          make -C ${{ matrix.service }} test-coverage
      - name: Check coverage output
        run: ls -la ${{ matrix.service }}/tests/coverage/
";
    assert_eq!(
        run_detect(wf),
        vec![
            "make -C api-service test-coverage",
            "make -C shared-utils test-coverage",
            "make -C portfolio-splitter test-coverage",
            "make -C raster-reader test-coverage",
            "make -C premise-reader test-coverage",
        ]
    );
}

#[test]
fn block_list_matrix_with_quotes() {
    let wf = "\
jobs:
  test:
    strategy:
      matrix:
        python:
          - \"3.11\"
          - '3.12'
    steps:
      - run: uv run --python ${{ matrix.python }} pytest -q
";
    assert_eq!(
        run_detect(wf),
        vec![
            "uv run --python 3.11 pytest -q",
            "uv run --python 3.12 pytest -q",
        ]
    );
}

#[test]
fn two_keys_give_cartesian_product() {
    let wf = "\
jobs:
  test:
    strategy:
      matrix:
        a: [x, y]
        b: [1, 2]
    steps:
      - run: make -C ${{ matrix.a }}${{matrix.b}} test
";
    assert_eq!(
        run_detect(wf),
        vec![
            "make -C x1 test",
            "make -C x2 test",
            "make -C y1 test",
            "make -C y2 test",
        ]
    );
}

#[test]
fn unknown_or_dynamic_matrix_key_is_dropped() {
    let wf = "\
jobs:
  test:
    strategy:
      matrix:
        include:
          - service: a
    steps:
      - run: make -C ${{ matrix.service }} test
      - run: uv run ruff check .
";
    assert_eq!(run_detect(wf), vec!["uv run ruff check ."]);
}

#[test]
fn matrix_is_scoped_to_its_job() {
    let wf = "\
jobs:
  first:
    strategy:
      matrix:
        service: [a]
    steps:
      - uses: actions/checkout@v4
  second:
    steps:
      - run: make -C ${{ matrix.service }} test
";
    assert!(run_detect(wf).is_empty());
}

#[test]
fn make_targets() {
    let wf = "\
jobs:
  test:
    steps:
      - run: make -C svc test-coverage
      - run: make --directory=svc test_unit
      - run: make -s lint
      - run: make -C svc lint-fix
      - run: make install
      - run: make -C a test lint
      - run: make FOO=1 test-x
";
    assert_eq!(
        run_detect(wf),
        vec![
            "make -C svc test-coverage",
            "make --directory=svc test_unit",
            "make -s lint",
        ]
    );
}

#[test]
fn duplicates_are_dropped() {
    let wf = "\
jobs:
  one:
    strategy:
      matrix:
        service: [a, b]
    steps:
      - run: make -C ${{ matrix.service }} test-coverage
  two:
    strategy:
      matrix:
        service: [a, b]
    steps:
      - run: make -C ${{ matrix.service }} test-coverage
";
    assert_eq!(
        run_detect(wf),
        vec!["make -C a test-coverage", "make -C b test-coverage"]
    );
}
