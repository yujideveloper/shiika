use std::path::PathBuf;
use std::rc::Rc;

#[derive(Debug, PartialEq, Clone)]
pub struct Location {
    pub line: usize,
    pub col: usize,
    pub pos: usize,
}

impl Location {
    pub fn new(line: usize, col: usize, pos: usize) -> Location {
        Location { line, col, pos }
    }
}

/// Range in a source file (end-exclusive)
#[derive(Debug, PartialEq, Clone)]
pub struct LocationSpan {
    pub filepath: Rc<PathBuf>,
    pub begin: Location,
    pub end: Location,
}

impl LocationSpan {
    pub fn new(filepath: &Rc<PathBuf>, begin: Location, end: Location) -> LocationSpan {
        if begin.pos > end.pos {
            println!(
                "[BUG] invalid LocationSpan (begin: {}, end: {})",
                begin.pos, end.pos
            );
            return LocationSpan::internal();
        }
        LocationSpan {
            filepath: filepath.clone(),
            begin,
            end,
        }
    }

    pub fn merge(begin: &LocationSpan, end: &LocationSpan) -> LocationSpan {
        if begin.filepath != end.filepath {
            println!(
                "[BUG] invalid LocationSpan (begin: {:?}, end: {:?})",
                begin, end
            );
            return LocationSpan::internal();
        }
        Self::new(&begin.filepath, begin.begin.clone(), end.end.clone())
    }

    pub fn begin_end(&self) -> (Location, Location) {
        (self.begin.clone(), self.end.clone())
    }

    /// Denotes that this ast or hir does not correspond to any source text.
    pub fn internal() -> LocationSpan {
        LocationSpan {
            filepath: Rc::new(PathBuf::from("<<internal>>")),
            begin: Location {
                line: 0,
                col: 0,
                pos: 0,
            },
            end: Location {
                line: 0,
                col: 0,
                pos: 0,
            },
        }
    }

    // TODO: remove this
    pub fn todo() -> LocationSpan {
        LocationSpan {
            filepath: Rc::new(PathBuf::new()),
            begin: Location {
                line: 0,
                col: 0,
                pos: 0,
            },
            end: Location {
                line: 0,
                col: 0,
                pos: 0,
            },
        }
    }
}
