use serde::{Deserialize, Serialize};
use shiika_core::{names::*, ty, ty::*};
use std::fmt;

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub struct MethodSignature {
    pub fullname: MethodFullname,
    pub ret_ty: TermTy,
    pub params: Vec<MethodParam>,
    pub typarams: Vec<TyParam>,
}

impl fmt::Display for MethodSignature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.full_string())
    }
}

impl MethodSignature {
    pub fn is_class_method(&self) -> bool {
        self.fullname.type_name.is_meta()
    }

    pub fn first_name(&self) -> &MethodFirstname {
        &self.fullname.first_name
    }

    /// If this method takes a block, returns types of block params and block value.
    pub fn block_ty(&self) -> Option<&[TermTy]> {
        self.params.last().and_then(|param| param.ty.fn_x_info())
    }

    /// Substitute type parameters with type arguments
    pub fn specialize(&self, class_tyargs: &[TermTy], method_tyargs: &[TermTy]) -> MethodSignature {
        MethodSignature {
            fullname: self.fullname.clone(),
            ret_ty: self.ret_ty.substitute(class_tyargs, method_tyargs),
            params: self
                .params
                .iter()
                .map(|param| param.substitute(class_tyargs, method_tyargs))
                .collect(),
            typarams: self.typarams.clone(), // eg. Array<T>#map<U>(f: Fn1<T, U>) -> Array<Int>#map<U>(f: Fn1<Int, U>)
        }
    }

    /// Returns true if `self` is the same as `other` except the
    /// parameter names.
    pub fn equivalent_to(&self, other: &MethodSignature) -> bool {
        if self.fullname.first_name != other.fullname.first_name {
            return false;
        }
        if !self.ret_ty.equals_to(&other.ret_ty) {
            return false;
        }
        if self.params.len() != other.params.len() {
            return false;
        }
        for i in 0..self.params.len() {
            if self.params[i].ty != other.params[i].ty {
                return false;
            }
        }
        if self.typarams != other.typarams {
            return false;
        }
        true
    }

    pub fn full_string(&self) -> String {
        let typarams = if self.typarams.is_empty() {
            "".to_string()
        } else {
            "<".to_string()
                + &self
                    .typarams
                    .iter()
                    .map(|x| format!("{}", &x.name))
                    .collect::<Vec<_>>()
                    .join(", ")
                + ">"
        };
        let params = self
            .params
            .iter()
            .map(|x| format!("{}: {}", &x.name, &x.ty))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "{}{}({}) -> {}",
            &self.fullname, typarams, params, &self.ret_ty
        )
    }
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub struct MethodParam {
    pub name: String,
    pub ty: TermTy,
}

impl MethodParam {
    pub fn substitute(&self, class_tyargs: &[TermTy], method_tyargs: &[TermTy]) -> MethodParam {
        MethodParam {
            name: self.name.clone(),
            ty: self.ty.substitute(class_tyargs, method_tyargs),
        }
    }
}

/// Return a param of the given name and its index
pub fn find_param<'a>(params: &'a [MethodParam], name: &str) -> Option<(usize, &'a MethodParam)> {
    params
        .iter()
        .enumerate()
        .find(|(_, param)| param.name == name)
}

/// Create a signature of a `new` method
pub fn signature_of_new(
    metaclass_fullname: &ClassFullname,
    initialize_params: Vec<MethodParam>,
    instance_ty: &TermTy,
) -> MethodSignature {
    MethodSignature {
        fullname: method_fullname(metaclass_fullname.clone().into(), "new"),
        ret_ty: instance_ty.clone(),
        params: initialize_params,
        typarams: vec![],
    }
}

/// Create a signature of a `initialize` method
pub fn signature_of_initialize(
    class_fullname: &ClassFullname,
    params: Vec<MethodParam>,
) -> MethodSignature {
    MethodSignature {
        fullname: method_fullname(class_fullname.clone().into(), "initialize"),
        ret_ty: ty::raw("Void"),
        params,
        typarams: vec![],
    }
}
