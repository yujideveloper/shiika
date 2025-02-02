use crate::names::class_fullname;
use crate::ty::lit_ty::LitTy;
use crate::ty::term_ty::{TermTy, TyBody};
use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Eq, Clone, Serialize, Deserialize)]
pub struct TyParamRef {
    pub kind: TyParamKind,
    pub name: String,
    pub idx: usize,
    pub upper_bound: LitTy,
    pub lower_bound: LitTy,
    /// Whether referring this typaram as a class object (eg. `p T`)
    pub as_class: bool,
}

#[derive(Debug, PartialEq, Eq, Clone, Serialize, Deserialize)]
pub enum TyParamKind {
    /// eg. `class A<B>`
    Class,
    /// eg. `def foo<X>(...)`
    Method,
}

impl From<TyParamRef> for TermTy {
    fn from(x: TyParamRef) -> Self {
        x.into_term_ty()
    }
}

impl TyParamRef {
    pub fn dbg_str(&self) -> String {
        let k = match &self.kind {
            TyParamKind::Class => "C",
            TyParamKind::Method => "M",
        };
        let c = if self.as_class { "!" } else { " " };
        format!("TyParamRef({}{}{}{})", &self.name, c, &self.idx, k)
    }

    pub fn to_term_ty(&self) -> TermTy {
        self.clone().into_term_ty()
    }

    pub fn into_term_ty(self) -> TermTy {
        TermTy {
            // TODO: self.name (eg. "T") is not a class name. Should remove fullname from TermTy?
            fullname: class_fullname(&self.name),
            body: TyBody::TyPara(self),
        }
    }

    /// Create new `TyParamRef` from self with as_class: true
    pub fn as_class(&self) -> TyParamRef {
        debug_assert!(!self.as_class);
        let mut ref2 = self.clone();
        ref2.as_class = true;
        ref2
    }

    /// Create new `TyParamRef` from self with as_class: false
    pub fn as_type(&self) -> TyParamRef {
        debug_assert!(self.as_class);
        let mut ref2 = self.clone();
        ref2.as_class = false;
        ref2
    }
}
