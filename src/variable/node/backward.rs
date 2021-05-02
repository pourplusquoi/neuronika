use super::{
    super::{BroadTensor, Broadcasted, DynTensor, Tensor},
    broadcasted_zeros,
    forward::{Data, Input},
    DotDim,
};
use ndarray::{
    concatenate,
    linalg::{general_mat_mul, general_mat_vec_mul},
    s, stack, Axis, DimMax, Dimension, Ix1, Ix2, NewAxis, RemoveAxis, Zip,
};
use std::cell::{Cell, Ref, RefCell, RefMut};
use std::rc::Rc;

// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~ Utility Functions ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

fn sum_axis_inplace(arr: &mut DynTensor, axis: Axis) {
    let (first, rest) = arr.view_mut().split_at(axis, 1);
    Zip::from(first.remove_axis(axis))
        .and(rest.lanes(axis))
        .for_each(|dst, src| *dst += src.sum());
    arr.index_axis_inplace(axis, 0);
}

pub fn reduce<D: Dimension, E: Dimension>(dest: &Tensor<D>, src: &Tensor<E>) -> DynTensor {
    let mut dyn_rhs = src.clone().into_dyn();

    unsafe {
        while (*(&dyn_rhs as *const DynTensor)).ndim() > dest.ndim() {
            sum_axis_inplace(&mut dyn_rhs, Axis(0));
        }
    }

    for (axis, size) in dest.shape().iter().enumerate() {
        if *size == 1 {
            sum_axis_inplace(&mut dyn_rhs, ndarray::Axis(axis));
            dyn_rhs.insert_axis_inplace(ndarray::Axis(axis));
        }
    }

    dyn_rhs
}

// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~ Traits ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

pub trait Gradient {
    type Dim: Dimension;

    fn gradient(&self) -> Ref<Tensor<Self::Dim>>;

    fn gradient_mut(&self) -> RefMut<Tensor<Self::Dim>>;
}

pub trait Overwrite {
    fn can_overwrite(&self) -> bool;

    fn set_overwrite(&self, state: bool);
}

pub trait Backward: Overwrite {
    fn backward(&self);
}

pub trait Differentiable {
    type Output: Gradient + Overwrite;

    fn differentiable(&self) -> Self::Output;
}

// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~ InputBackward ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

pub struct InputBackward<D: Dimension> {
    gradient: RefCell<Tensor<D>>,
    overwrite: Cell<bool>,
}

impl<D: Dimension> InputBackward<D> {
    pub fn zero_grad(&self) {
        self.gradient.borrow_mut().map_inplace(|el| *el = 0.0);
    }
}

impl<D: Dimension> Gradient for InputBackward<D> {
    type Dim = D;

    fn gradient(&self) -> Ref<Tensor<Self::Dim>> {
        self.gradient.borrow()
    }

    fn gradient_mut(&self) -> RefMut<Tensor<Self::Dim>> {
        self.gradient.borrow_mut()
    }
}

impl<D: Dimension> Differentiable for Input<D> {
    type Output = InputBackward<D>;

    fn differentiable(&self) -> Self::Output {
        Self::Output {
            gradient: RefCell::new(Tensor::zeros(self.data().raw_dim())),
            overwrite: Cell::new(true),
        }
    }
}

impl<D: Dimension> Overwrite for InputBackward<D> {
    fn can_overwrite(&self) -> bool {
        self.overwrite.get()
    }

    fn set_overwrite(&self, state: bool) {
        self.overwrite.set(state);
    }
}

// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~ NegationBackward ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

pub struct NegationBackward<T: Gradient + Overwrite> {
    operand: Rc<T>,
    gradient: RefCell<Tensor<T::Dim>>,
    overwrite: Cell<bool>,
}

impl<T: Gradient + Overwrite> NegationBackward<T> {
    pub fn new(operand: Rc<T>) -> Self {
        let gradient = RefCell::new(Tensor::zeros(operand.gradient().raw_dim()));

        Self {
            operand,
            gradient,
            overwrite: Cell::new(true),
        }
    }
}

impl<T: Gradient + Overwrite> Gradient for NegationBackward<T> {
    type Dim = T::Dim;

    fn gradient(&self) -> Ref<Tensor<Self::Dim>> {
        self.gradient.borrow()
    }

    fn gradient_mut(&self) -> RefMut<Tensor<Self::Dim>> {
        self.gradient.borrow_mut()
    }
}

impl<T: Gradient + Overwrite> Backward for NegationBackward<T> {
    fn backward(&self) {
        let operand = &mut *self.operand.gradient_mut();
        let gradient = &*self.gradient.borrow();
        let zip = Zip::from(operand).and(gradient);
        if self.operand.can_overwrite() {
            zip.par_for_each(|dest, src| *dest = -src);
            self.operand.set_overwrite(false);
        } else {
            zip.par_for_each(|dest, src| *dest -= src);
        }
    }
}

impl<T: Gradient + Overwrite> Overwrite for NegationBackward<T> {
    fn can_overwrite(&self) -> bool {
        self.overwrite.get()
    }

    fn set_overwrite(&self, state: bool) {
        self.overwrite.set(state);
    }
}

// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~ TransposeBackward ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

pub struct TransposeBackward<T: Gradient + Overwrite> {
    operand: Rc<T>,
    gradient: RefCell<Tensor<T::Dim>>,
    overwrite: Cell<bool>,
}

impl<T: Gradient + Overwrite> TransposeBackward<T> {
    pub fn new(operand: Rc<T>) -> Self {
        let gradient = RefCell::new(Tensor::zeros(operand.gradient().t().raw_dim()));

        Self {
            operand,
            gradient,
            overwrite: Cell::new(true),
        }
    }
}

impl<T: Gradient + Overwrite> Gradient for TransposeBackward<T> {
    type Dim = T::Dim;

    fn gradient(&self) -> Ref<Tensor<Self::Dim>> {
        self.gradient.borrow()
    }

    fn gradient_mut(&self) -> RefMut<Tensor<Self::Dim>> {
        self.gradient.borrow_mut()
    }
}

impl<T: Gradient + Overwrite> Backward for TransposeBackward<T> {
    fn backward(&self) {
        let operand = &mut *self.operand.gradient_mut();
        let gradient = &*self.gradient.borrow();
        let zip = Zip::from(operand).and(gradient.t());
        if self.operand.can_overwrite() {
            self.operand.set_overwrite(false);
            zip.par_for_each(|dest, src| *dest = *src);
        } else {
            zip.par_for_each(|dest, src| *dest += *src);
        }
    }
}

impl<T: Gradient + Overwrite> Overwrite for TransposeBackward<T> {
    fn can_overwrite(&self) -> bool {
        self.overwrite.get()
    }

    fn set_overwrite(&self, state: bool) {
        self.overwrite.set(state);
    }
}

// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~ AdditionBackward ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

pub struct AdditionBackward<Lhs, Rhs>
where
    Lhs: Gradient + Overwrite,
    Rhs: Gradient + Overwrite,
    Lhs::Dim: Dimension + DimMax<Rhs::Dim>,
{
    left: Rc<Lhs>,
    right: Rc<Rhs>,
    gradient: RefCell<Tensor<Broadcasted<Lhs::Dim, Rhs::Dim>>>,
    overwrite: Cell<bool>,
}

impl<Lhs, Rhs> AdditionBackward<Lhs, Rhs>
where
    Lhs: Gradient + Overwrite,
    Rhs: Gradient + Overwrite,
    Lhs::Dim: Dimension + DimMax<Rhs::Dim>,
{
    pub fn new(left: Rc<Lhs>, right: Rc<Rhs>) -> Self {
        let gradient = RefCell::new(broadcasted_zeros(&left.gradient(), &right.gradient()));

        Self {
            left,
            right,
            gradient,
            overwrite: Cell::new(true),
        }
    }
}

impl<Lhs, Rhs> Backward for AdditionBackward<Lhs, Rhs>
where
    Lhs: Gradient + Overwrite,
    Rhs: Gradient + Overwrite,
    Lhs::Dim: Dimension + DimMax<Rhs::Dim>,
{
    fn backward(&self) {
        let (mut lhs_grad, mut rhs_grad) =
            { (self.left.gradient_mut(), self.right.gradient_mut()) };
        let (gradient_lhs, gradient_rhs) = {
            let grad = self.gradient.borrow();
            (reduce(&*lhs_grad, &grad), reduce(&*rhs_grad, &grad))
        };

        if self.left.can_overwrite() {
            Zip::from(&mut *lhs_grad)
                .and_broadcast(&gradient_lhs.as_standard_layout())
                .par_for_each(|dest, src| *dest = *src);
            self.left.set_overwrite(false);
        } else {
            Zip::from(&mut *lhs_grad)
                .and_broadcast(&gradient_lhs.as_standard_layout())
                .par_for_each(|dest, src| *dest += *src);
        }

        if self.right.can_overwrite() {
            Zip::from(&mut *rhs_grad)
                .and_broadcast(&gradient_rhs.as_standard_layout())
                .par_for_each(|dest, src| *dest = *src);
            self.right.set_overwrite(false);
        } else {
            Zip::from(&mut *rhs_grad)
                .and_broadcast(&gradient_rhs.as_standard_layout())
                .par_for_each(|dest, src| *dest += *src);
        }
    }
}

impl<Lhs, Rhs> Gradient for AdditionBackward<Lhs, Rhs>
where
    Lhs: Gradient + Overwrite,
    Rhs: Gradient + Overwrite,
    Lhs::Dim: Dimension + DimMax<Rhs::Dim>,
{
    type Dim = Broadcasted<Lhs::Dim, Rhs::Dim>;

    fn gradient(&self) -> Ref<Tensor<Self::Dim>> {
        self.gradient.borrow()
    }

    fn gradient_mut(&self) -> RefMut<Tensor<Self::Dim>> {
        self.gradient.borrow_mut()
    }
}

impl<Lhs, Rhs> Overwrite for AdditionBackward<Lhs, Rhs>
where
    Lhs: Gradient + Overwrite,
    Rhs: Gradient + Overwrite,
    Lhs::Dim: Dimension + DimMax<Rhs::Dim>,
{
    fn can_overwrite(&self) -> bool {
        self.overwrite.get()
    }

    fn set_overwrite(&self, state: bool) {
        self.overwrite.set(state);
    }
}

// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~ AdditionBackwardUnary ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

pub struct AdditionBackwardUnary<T, U>
where
    T: Gradient + Overwrite,
    U: Data,
    T::Dim: Dimension + DimMax<U::Dim>,
{
    diff_operand: Rc<T>,
    gradient: RefCell<BroadTensor<T::Dim, U::Dim>>,
    overwrite: Cell<bool>,
}

impl<T, U> AdditionBackwardUnary<T, U>
where
    T: Gradient + Overwrite,
    U: Data,
    T::Dim: Dimension + DimMax<U::Dim>,
{
    pub fn new(diff_operand: Rc<T>, no_diff_operand: Rc<U>) -> Self {
        let gradient = RefCell::new(broadcasted_zeros(
            &*diff_operand.gradient(),
            &*no_diff_operand.data(),
        ));

        Self {
            diff_operand,
            gradient,
            overwrite: Cell::new(true),
        }
    }
}

impl<T, U> Backward for AdditionBackwardUnary<T, U>
where
    T: Gradient + Overwrite,
    U: Data,
    T::Dim: Dimension + DimMax<U::Dim>,
{
    fn backward(&self) {
        let mut operand_grad = self.diff_operand.gradient_mut();
        let gradient = reduce(&operand_grad, &*self.gradient.borrow());
        if self.diff_operand.can_overwrite() {
            Zip::from(&mut *operand_grad)
                .and_broadcast(&gradient.as_standard_layout())
                .par_for_each(|dest, src| *dest = *src);
            self.diff_operand.set_overwrite(false);
        } else {
            Zip::from(&mut *operand_grad)
                .and_broadcast(&gradient.as_standard_layout())
                .par_for_each(|dest, src| *dest += *src);
        }
    }
}

impl<T, U> Gradient for AdditionBackwardUnary<T, U>
where
    T: Gradient + Overwrite,
    U: Data,
    T::Dim: Dimension + DimMax<U::Dim>,
{
    type Dim = Broadcasted<T::Dim, U::Dim>;

    fn gradient(&self) -> Ref<Tensor<Self::Dim>> {
        self.gradient.borrow()
    }

    fn gradient_mut(&self) -> RefMut<Tensor<Self::Dim>> {
        self.gradient.borrow_mut()
    }
}

impl<T, U> Overwrite for AdditionBackwardUnary<T, U>
where
    T: Gradient + Overwrite,
    U: Data,
    T::Dim: Dimension + DimMax<U::Dim>,
{
    fn can_overwrite(&self) -> bool {
        self.overwrite.get()
    }

    fn set_overwrite(&self, state: bool) {
        self.overwrite.set(state);
    }
}

// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~ SubtractionBackward ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

pub struct SubtractionBackward<Lhs, Rhs>
where
    Lhs: Gradient + Overwrite,
    Rhs: Gradient + Overwrite,
    Lhs::Dim: Dimension + DimMax<Rhs::Dim>,
{
    left: Rc<Lhs>,
    right: Rc<Rhs>,
    gradient: RefCell<BroadTensor<Lhs::Dim, Rhs::Dim>>,
    overwrite: Cell<bool>,
}

impl<Lhs, Rhs> SubtractionBackward<Lhs, Rhs>
where
    Lhs: Gradient + Overwrite,
    Rhs: Gradient + Overwrite,
    Lhs::Dim: Dimension + DimMax<Rhs::Dim>,
{
    pub fn new(left: Rc<Lhs>, right: Rc<Rhs>) -> Self {
        let gradient = RefCell::new(broadcasted_zeros(&left.gradient(), &right.gradient()));

        Self {
            left,
            right,
            gradient,
            overwrite: Cell::new(true),
        }
    }
}

impl<Lhs, Rhs> Backward for SubtractionBackward<Lhs, Rhs>
where
    Lhs: Gradient + Overwrite,
    Rhs: Gradient + Overwrite,
    Lhs::Dim: Dimension + DimMax<Rhs::Dim>,
{
    fn backward(&self) {
        let (mut lhs_grad, mut rhs_grad) =
            { (self.left.gradient_mut(), self.right.gradient_mut()) };
        let (gradient_lhs, gradient_rhs) = {
            let grad = self.gradient.borrow();
            (reduce(&*lhs_grad, &grad), reduce(&*rhs_grad, &grad))
        };

        if self.left.can_overwrite() {
            Zip::from(&mut *lhs_grad)
                .and_broadcast(&gradient_lhs.as_standard_layout())
                .par_for_each(|dest, src| *dest = *src);
            self.left.set_overwrite(false);
        } else {
            Zip::from(&mut *lhs_grad)
                .and_broadcast(&gradient_lhs.as_standard_layout())
                .par_for_each(|dest, src| *dest += *src);
        }

        if self.right.can_overwrite() {
            Zip::from(&mut *rhs_grad)
                .and_broadcast(&gradient_rhs.as_standard_layout())
                .par_for_each(|dest, src| *dest = -src);
            self.right.set_overwrite(false);
        } else {
            Zip::from(&mut *rhs_grad)
                .and_broadcast(&gradient_rhs.as_standard_layout())
                .par_for_each(|dest, src| *dest += -src);
        }
    }
}

impl<Lhs, Rhs> Gradient for SubtractionBackward<Lhs, Rhs>
where
    Lhs: Gradient + Overwrite,
    Rhs: Gradient + Overwrite,
    Lhs::Dim: Dimension + DimMax<Rhs::Dim>,
{
    type Dim = Broadcasted<Lhs::Dim, Rhs::Dim>;

    fn gradient(&self) -> Ref<Tensor<Self::Dim>> {
        self.gradient.borrow()
    }

    fn gradient_mut(&self) -> RefMut<Tensor<Self::Dim>> {
        self.gradient.borrow_mut()
    }
}

impl<Lhs, Rhs> Overwrite for SubtractionBackward<Lhs, Rhs>
where
    Lhs: Gradient + Overwrite,
    Rhs: Gradient + Overwrite,
    Lhs::Dim: Dimension + DimMax<Rhs::Dim>,
{
    fn can_overwrite(&self) -> bool {
        self.overwrite.get()
    }

    fn set_overwrite(&self, state: bool) {
        self.overwrite.set(state);
    }
}

// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~ SubtractionBackwardLeft ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

pub struct SubtractionBackwardLeft<T, U>
where
    T: Gradient + Overwrite,
    U: Data,
    T::Dim: Dimension + DimMax<U::Dim>,
{
    diff_operand: Rc<T>,
    gradient: RefCell<BroadTensor<T::Dim, U::Dim>>,
    overwrite: Cell<bool>,
}

impl<T, U> SubtractionBackwardLeft<T, U>
where
    T: Gradient + Overwrite,
    U: Data,
    T::Dim: Dimension + DimMax<U::Dim>,
{
    pub fn new(diff_operand: Rc<T>, operand: Rc<U>) -> Self {
        let gradient = RefCell::new(broadcasted_zeros(
            &*diff_operand.gradient(),
            &*operand.data(),
        ));

        Self {
            diff_operand,
            gradient,
            overwrite: Cell::new(true),
        }
    }
}

impl<T, U> Backward for SubtractionBackwardLeft<T, U>
where
    T: Gradient + Overwrite,
    U: Data,
    T::Dim: Dimension + DimMax<U::Dim>,
{
    fn backward(&self) {
        let mut operand_grad = self.diff_operand.gradient_mut();
        let gradient = reduce(&operand_grad, &*self.gradient.borrow());

        if self.diff_operand.can_overwrite() {
            Zip::from(&mut *operand_grad)
                .and_broadcast(&gradient)
                .par_for_each(|dest, src| *dest = *src);
            self.diff_operand.set_overwrite(false);
        } else {
            Zip::from(&mut *operand_grad)
                .and_broadcast(&gradient)
                .par_for_each(|dest, src| *dest += *src);
        }
    }
}

impl<T, U> Gradient for SubtractionBackwardLeft<T, U>
where
    T: Gradient + Overwrite,
    U: Data,
    T::Dim: Dimension + DimMax<U::Dim>,
{
    type Dim = Broadcasted<T::Dim, U::Dim>;

    fn gradient(&self) -> Ref<Tensor<Self::Dim>> {
        self.gradient.borrow()
    }

    fn gradient_mut(&self) -> RefMut<Tensor<Self::Dim>> {
        self.gradient.borrow_mut()
    }
}

impl<T, U> Overwrite for SubtractionBackwardLeft<T, U>
where
    T: Gradient + Overwrite,
    U: Data,
    T::Dim: Dimension + DimMax<U::Dim>,
{
    fn can_overwrite(&self) -> bool {
        self.overwrite.get()
    }

    fn set_overwrite(&self, state: bool) {
        self.overwrite.set(state);
    }
}

// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~ SubtractionBackwardRight ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

pub struct SubtractionBackwardRight<T, U>
where
    T: Gradient + Overwrite,
    U: Data,
    T::Dim: Dimension + DimMax<U::Dim>,
{
    diff_operand: Rc<T>,
    gradient: RefCell<BroadTensor<T::Dim, U::Dim>>,
    overwrite: Cell<bool>,
}

impl<T, U> SubtractionBackwardRight<T, U>
where
    T: Gradient + Overwrite,
    U: Data,
    T::Dim: Dimension + DimMax<U::Dim>,
{
    pub fn new(diff_operand: Rc<T>, operand: Rc<U>) -> Self {
        let gradient = RefCell::new(broadcasted_zeros(
            &*diff_operand.gradient(),
            &*operand.data(),
        ));

        Self {
            diff_operand,
            gradient,
            overwrite: Cell::new(true),
        }
    }
}

impl<T, U> Backward for SubtractionBackwardRight<T, U>
where
    T: Gradient + Overwrite,
    U: Data,
    T::Dim: Dimension + DimMax<U::Dim>,
{
    fn backward(&self) {
        self.overwrite.set(true);
        let mut operand_grad = self.diff_operand.gradient_mut();
        let gradient = reduce(&operand_grad, &*self.gradient.borrow());

        if self.diff_operand.can_overwrite() {
            Zip::from(&mut *operand_grad)
                .and_broadcast(&gradient)
                .par_for_each(|dest, src| *dest = -src);
            self.diff_operand.set_overwrite(false);
        } else {
            Zip::from(&mut *operand_grad)
                .and_broadcast(&gradient)
                .par_for_each(|dest, src| *dest += -src);
        }
    }
}

impl<T, U> Gradient for SubtractionBackwardRight<T, U>
where
    T: Gradient + Overwrite,
    U: Data,
    T::Dim: Dimension + DimMax<U::Dim>,
{
    type Dim = Broadcasted<T::Dim, U::Dim>;

    fn gradient(&self) -> Ref<Tensor<Self::Dim>> {
        self.gradient.borrow()
    }

    fn gradient_mut(&self) -> RefMut<Tensor<Self::Dim>> {
        self.gradient.borrow_mut()
    }
}

impl<T, U> Overwrite for SubtractionBackwardRight<T, U>
where
    T: Gradient + Overwrite,
    U: Data,
    T::Dim: Dimension + DimMax<U::Dim>,
{
    fn can_overwrite(&self) -> bool {
        self.overwrite.get()
    }

    fn set_overwrite(&self, state: bool) {
        self.overwrite.set(state);
    }
}

// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~ MultiplicationBackward ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

pub struct MultiplicationBackward<LhsD, LhsG, RhsD, RhsG>
where
    LhsD: Data,
    RhsD: Data,
    LhsG: Gradient + Overwrite,
    RhsG: Gradient + Overwrite,
    LhsD::Dim: Dimension + DimMax<RhsD::Dim>,
    LhsG::Dim: Dimension + DimMax<RhsG::Dim>,
{
    left_data: Rc<LhsD>,
    left_grad: Rc<LhsG>,
    right_data: Rc<RhsD>,
    right_grad: Rc<RhsG>,
    gradient: RefCell<Tensor<Broadcasted<LhsG::Dim, RhsG::Dim>>>,
    overwrite: Cell<bool>,
}

impl<LhsD, LhsG, RhsD, RhsG> MultiplicationBackward<LhsD, LhsG, RhsD, RhsG>
where
    LhsD: Data,
    RhsD: Data,
    LhsG: Gradient + Overwrite,
    RhsG: Gradient + Overwrite,
    LhsD::Dim: Dimension + DimMax<RhsD::Dim>,
    LhsG::Dim: Dimension + DimMax<RhsG::Dim>,
{
    pub fn new(
        left_data: Rc<LhsD>,
        left_grad: Rc<LhsG>,
        right_data: Rc<RhsD>,
        right_grad: Rc<RhsG>,
    ) -> Self {
        let gradient = RefCell::new(broadcasted_zeros(
            &left_grad.gradient(),
            &right_grad.gradient(),
        ));

        Self {
            left_data,
            left_grad,
            right_data,
            right_grad,
            gradient,
            overwrite: Cell::new(true),
        }
    }
}

impl<LhsD, LhsG, RhsD, RhsG> Backward for MultiplicationBackward<LhsD, LhsG, RhsD, RhsG>
where
    LhsD: Data,
    RhsD: Data,
    LhsG: Gradient + Overwrite,
    RhsG: Gradient + Overwrite,
    LhsD::Dim: Dimension + DimMax<RhsD::Dim>,
    LhsG::Dim: Dimension + DimMax<RhsG::Dim>,
{
    fn backward(&self) {
        let (mut lhs_grad, mut rhs_grad) = {
            (
                self.left_grad.gradient_mut(),
                self.right_grad.gradient_mut(),
            )
        };
        let grad = self.gradient.borrow();

        let mut tmp = Tensor::zeros(grad.raw_dim());
        Zip::from(&mut tmp)
            .and(&*grad)
            .and_broadcast(&*self.right_data.data())
            .par_for_each(|tmp_el, grad_el, rhs_data_el| *tmp_el = grad_el * rhs_data_el);

        let to_left_grad = reduce(&*lhs_grad, &tmp);
        if self.left_grad.can_overwrite() {
            Zip::from(&mut *lhs_grad)
                .and_broadcast(&to_left_grad.as_standard_layout())
                .par_for_each(|dest, src| *dest = *src);
            self.left_grad.set_overwrite(false);
        } else {
            Zip::from(&mut *lhs_grad)
                .and_broadcast(&to_left_grad.as_standard_layout())
                .par_for_each(|dest, src| *dest += *src);
        }

        Zip::from(&mut tmp)
            .and(&*grad)
            .and_broadcast(&*self.left_data.data())
            .par_for_each(|tmp_el, grad_el, lhs_data_el| *tmp_el = grad_el * lhs_data_el);

        let to_right_grad = reduce(&*rhs_grad, &tmp);
        if self.right_grad.can_overwrite() {
            Zip::from(&mut *rhs_grad)
                .and_broadcast(&to_right_grad.as_standard_layout())
                .par_for_each(|dest, src| *dest = *src);
            self.right_grad.set_overwrite(false);
        } else {
            Zip::from(&mut *rhs_grad)
                .and_broadcast(&to_right_grad.as_standard_layout())
                .par_for_each(|dest, src| *dest += *src);
        }
    }
}

impl<LhsD, LhsG, RhsD, RhsG> Gradient for MultiplicationBackward<LhsD, LhsG, RhsD, RhsG>
where
    LhsD: Data,
    RhsD: Data,
    LhsG: Gradient + Overwrite,
    RhsG: Gradient + Overwrite,
    LhsD::Dim: Dimension + DimMax<RhsD::Dim>,
    LhsG::Dim: Dimension + DimMax<RhsG::Dim>,
{
    type Dim = Broadcasted<LhsG::Dim, RhsG::Dim>;

    fn gradient(&self) -> Ref<Tensor<Self::Dim>> {
        self.gradient.borrow()
    }

    fn gradient_mut(&self) -> RefMut<Tensor<Self::Dim>> {
        self.gradient.borrow_mut()
    }
}

impl<LhsD, LhsG, RhsD, RhsG> Overwrite for MultiplicationBackward<LhsD, LhsG, RhsD, RhsG>
where
    LhsD: Data,
    RhsD: Data,
    LhsG: Gradient + Overwrite,
    RhsG: Gradient + Overwrite,
    LhsD::Dim: Dimension + DimMax<RhsD::Dim>,
    LhsG::Dim: Dimension + DimMax<RhsG::Dim>,
{
    fn can_overwrite(&self) -> bool {
        self.overwrite.get()
    }

    fn set_overwrite(&self, state: bool) {
        self.overwrite.set(state);
    }
}

// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~ MultiplicationBackwardUnary ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

pub struct MultiplicationBackwardUnary<T, U>
where
    T: Gradient + Overwrite,
    U: Data,
    T::Dim: Dimension + DimMax<U::Dim>,
{
    diff_operand: Rc<T>,
    no_diff_operand: Rc<U>,
    gradient: RefCell<BroadTensor<T::Dim, U::Dim>>,
    overwrite: Cell<bool>,
}

impl<T, U> MultiplicationBackwardUnary<T, U>
where
    T: Gradient + Overwrite,
    U: Data,
    T::Dim: Dimension + DimMax<U::Dim>,
{
    pub fn new(diff_operand: Rc<T>, no_diff_operand: Rc<U>) -> Self {
        let gradient = RefCell::new(broadcasted_zeros(
            &*diff_operand.gradient(),
            &*no_diff_operand.data(),
        ));

        Self {
            diff_operand,
            no_diff_operand,
            gradient,
            overwrite: Cell::new(true),
        }
    }
}

impl<T, U> Backward for MultiplicationBackwardUnary<T, U>
where
    T: Gradient + Overwrite,
    U: Data,
    T::Dim: Dimension + DimMax<U::Dim>,
{
    fn backward(&self) {
        let mut operand_grad = self.diff_operand.gradient_mut();
        let grad = self.gradient.borrow();

        let mut tmp = Tensor::zeros(grad.raw_dim());
        Zip::from(&mut tmp)
            .and(&*grad)
            .and_broadcast(&*self.no_diff_operand.data())
            .par_for_each(|tmp_el, grad_el, no_diff_operand_el| {
                *tmp_el = grad_el * no_diff_operand_el
            });

        let gradient = reduce(&operand_grad, &tmp);
        if self.diff_operand.can_overwrite() {
            Zip::from(&mut *operand_grad)
                .and_broadcast(&gradient.as_standard_layout())
                .par_for_each(|dest, src| *dest = *src);
            self.diff_operand.set_overwrite(false);
        } else {
            Zip::from(&mut *operand_grad)
                .and_broadcast(&gradient.as_standard_layout())
                .par_for_each(|dest, src| *dest += *src);
        }
    }
}

impl<T, U> Gradient for MultiplicationBackwardUnary<T, U>
where
    T: Gradient + Overwrite,
    U: Data,
    T::Dim: Dimension + DimMax<U::Dim>,
{
    type Dim = Broadcasted<T::Dim, U::Dim>;

    fn gradient(&self) -> Ref<Tensor<Self::Dim>> {
        self.gradient.borrow()
    }

    fn gradient_mut(&self) -> RefMut<Tensor<Self::Dim>> {
        self.gradient.borrow_mut()
    }
}

impl<T, U> Overwrite for MultiplicationBackwardUnary<T, U>
where
    T: Gradient + Overwrite,
    U: Data,
    T::Dim: Dimension + DimMax<U::Dim>,
{
    fn can_overwrite(&self) -> bool {
        self.overwrite.get()
    }

    fn set_overwrite(&self, state: bool) {
        self.overwrite.set(state);
    }
}

// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~ DivisionBackward ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

pub struct DivisionBackward<LhsD, LhsG, RhsD, RhsG>
where
    LhsD: Data,
    RhsD: Data,
    LhsG: Gradient + Overwrite,
    RhsG: Gradient + Overwrite,
    LhsD::Dim: Dimension + DimMax<RhsD::Dim>,
    LhsG::Dim: Dimension + DimMax<RhsG::Dim>,
{
    left_data: Rc<LhsD>,
    left_grad: Rc<LhsG>,
    right_data: Rc<RhsD>,
    right_grad: Rc<RhsG>,
    gradient: RefCell<BroadTensor<LhsG::Dim, RhsG::Dim>>,
    overwrite: Cell<bool>,
}

impl<LhsD, LhsG, RhsD, RhsG> DivisionBackward<LhsD, LhsG, RhsD, RhsG>
where
    LhsD: Data,
    RhsD: Data,
    LhsG: Gradient + Overwrite,
    RhsG: Gradient + Overwrite,
    LhsD::Dim: Dimension + DimMax<RhsD::Dim>,
    LhsG::Dim: Dimension + DimMax<RhsG::Dim>,
{
    pub fn new(
        left_data: Rc<LhsD>,
        left_grad: Rc<LhsG>,
        right_data: Rc<RhsD>,
        right_grad: Rc<RhsG>,
    ) -> Self {
        let gradient = RefCell::new(broadcasted_zeros(
            &left_grad.gradient(),
            &right_grad.gradient(),
        ));

        Self {
            left_data,
            left_grad,
            right_data,
            right_grad,
            gradient,
            overwrite: Cell::new(true),
        }
    }
}

impl<LhsD, LhsG, RhsD, RhsG> Backward for DivisionBackward<LhsD, LhsG, RhsD, RhsG>
where
    LhsD: Data,
    RhsD: Data,
    LhsG: Gradient + Overwrite,
    RhsG: Gradient + Overwrite,
    LhsD::Dim: Dimension + DimMax<RhsD::Dim>,
    LhsG::Dim: Dimension + DimMax<RhsG::Dim>,
{
    fn backward(&self) {
        let (mut lhs_grad, mut rhs_grad) = {
            (
                self.left_grad.gradient_mut(),
                self.right_grad.gradient_mut(),
            )
        };
        let grad = self.gradient.borrow();

        let mut tmp = Tensor::zeros(grad.raw_dim());
        Zip::from(&mut tmp)
            .and(&*grad)
            .and_broadcast(&*self.right_data.data())
            .par_for_each(|tmp_el, grad_el, rhs_data_el| *tmp_el = grad_el / rhs_data_el);

        let to_left_grad = reduce(&*lhs_grad, &tmp);
        if self.left_grad.can_overwrite() {
            Zip::from(&mut *lhs_grad)
                .and_broadcast(&to_left_grad.as_standard_layout())
                .par_for_each(|dest, src| *dest = *src);
            self.left_grad.set_overwrite(false);
        } else {
            Zip::from(&mut *lhs_grad)
                .and_broadcast(&to_left_grad.as_standard_layout())
                .par_for_each(|dest, src| *dest += *src);
        }

        Zip::from(&mut tmp)
            .and(&*grad)
            .and_broadcast(&*self.left_data.data())
            .and_broadcast(&*self.right_data.data())
            .par_for_each(|tmp_el, grad_el, lhs_data_el, rhs_data_el| {
                *tmp_el = -grad_el * lhs_data_el / rhs_data_el.powi(2)
            });

        let to_right_grad = reduce(&*rhs_grad, &tmp);
        if self.right_grad.can_overwrite() {
            Zip::from(&mut *rhs_grad)
                .and_broadcast(&to_right_grad.as_standard_layout())
                .par_for_each(|dest, src| *dest = *src);
            self.right_grad.set_overwrite(false);
        } else {
            Zip::from(&mut *rhs_grad)
                .and_broadcast(&to_right_grad.as_standard_layout())
                .par_for_each(|dest, src| *dest += *src);
        }
    }
}

impl<LhsD, LhsG, RhsD, RhsG> Gradient for DivisionBackward<LhsD, LhsG, RhsD, RhsG>
where
    LhsD: Data,
    RhsD: Data,
    LhsG: Gradient + Overwrite,
    RhsG: Gradient + Overwrite,
    LhsD::Dim: Dimension + DimMax<RhsD::Dim>,
    LhsG::Dim: Dimension + DimMax<RhsG::Dim>,
{
    type Dim = Broadcasted<LhsG::Dim, RhsG::Dim>;

    fn gradient(&self) -> Ref<Tensor<Self::Dim>> {
        self.gradient.borrow()
    }

    fn gradient_mut(&self) -> RefMut<Tensor<Self::Dim>> {
        self.gradient.borrow_mut()
    }
}

impl<LhsD, LhsG, RhsD, RhsG> Overwrite for DivisionBackward<LhsD, LhsG, RhsD, RhsG>
where
    LhsD: Data,
    RhsD: Data,
    LhsG: Gradient + Overwrite,
    RhsG: Gradient + Overwrite,
    LhsD::Dim: Dimension + DimMax<RhsD::Dim>,
    LhsG::Dim: Dimension + DimMax<RhsG::Dim>,
{
    fn can_overwrite(&self) -> bool {
        self.overwrite.get()
    }

    fn set_overwrite(&self, state: bool) {
        self.overwrite.set(state);
    }
}

// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~ DivisionBackwardLeft ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

pub struct DivisionBackwardLeft<LhsG, RhsD>
where
    RhsD: Data,
    LhsG: Gradient + Overwrite,
    LhsG::Dim: Dimension + DimMax<RhsD::Dim>,
{
    left_grad: Rc<LhsG>,
    right_data: Rc<RhsD>,
    gradient: RefCell<Tensor<Broadcasted<LhsG::Dim, RhsD::Dim>>>,
    overwrite: Cell<bool>,
}

impl<LhsG, RhsD> DivisionBackwardLeft<LhsG, RhsD>
where
    RhsD: Data,
    LhsG: Gradient + Overwrite,
    LhsG::Dim: Dimension + DimMax<RhsD::Dim>,
{
    pub fn new(left_grad: Rc<LhsG>, right_data: Rc<RhsD>) -> Self {
        let gradient = RefCell::new(broadcasted_zeros(&left_grad.gradient(), &right_data.data()));

        Self {
            left_grad,
            right_data,
            gradient,
            overwrite: Cell::new(true),
        }
    }
}

impl<LhsG, RhsD> Backward for DivisionBackwardLeft<LhsG, RhsD>
where
    RhsD: Data,
    LhsG: Gradient + Overwrite,
    LhsG::Dim: Dimension + DimMax<RhsD::Dim>,
{
    fn backward(&self) {
        let mut lhs_grad = { self.left_grad.gradient_mut() };
        let grad = self.gradient.borrow();

        let mut tmp = Tensor::zeros(grad.raw_dim());
        Zip::from(&mut tmp)
            .and(&*grad)
            .and_broadcast(&*self.right_data.data())
            .par_for_each(|tmp_el, grad_el, rhs_data_el| *tmp_el = grad_el / rhs_data_el);

        let to_left_grad = reduce(&*lhs_grad, &tmp);
        if self.left_grad.can_overwrite() {
            Zip::from(&mut *lhs_grad)
                .and_broadcast(&to_left_grad.as_standard_layout())
                .par_for_each(|dest, src| *dest = *src);
            self.left_grad.set_overwrite(false);
        } else {
            Zip::from(&mut *lhs_grad)
                .and_broadcast(&to_left_grad.as_standard_layout())
                .par_for_each(|dest, src| *dest += *src);
        }
    }
}

impl<LhsG, RhsD> Gradient for DivisionBackwardLeft<LhsG, RhsD>
where
    RhsD: Data,
    LhsG: Gradient + Overwrite,
    LhsG::Dim: Dimension + DimMax<RhsD::Dim>,
{
    type Dim = Broadcasted<LhsG::Dim, RhsD::Dim>;

    fn gradient(&self) -> Ref<Tensor<Self::Dim>> {
        self.gradient.borrow()
    }

    fn gradient_mut(&self) -> RefMut<Tensor<Self::Dim>> {
        self.gradient.borrow_mut()
    }
}

impl<LhsG, RhsD> Overwrite for DivisionBackwardLeft<LhsG, RhsD>
where
    RhsD: Data,
    LhsG: Gradient + Overwrite,
    LhsG::Dim: Dimension + DimMax<RhsD::Dim>,
{
    fn can_overwrite(&self) -> bool {
        self.overwrite.get()
    }

    fn set_overwrite(&self, state: bool) {
        self.overwrite.set(state);
    }
}

// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~ DivisionBackwardRight ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

pub struct DivisionBackwardRight<LhsD, RhsD, RhsG>
where
    LhsD: Data,
    RhsD: Data,
    RhsG: Gradient + Overwrite,
    LhsD::Dim: Dimension + DimMax<RhsG::Dim>,
{
    left_data: Rc<LhsD>,
    right_data: Rc<RhsD>,
    right_grad: Rc<RhsG>,
    gradient: RefCell<Tensor<Broadcasted<LhsD::Dim, RhsG::Dim>>>,
    overwrite: Cell<bool>,
}

impl<LhsD, RhsD, RhsG> DivisionBackwardRight<LhsD, RhsD, RhsG>
where
    LhsD: Data,
    RhsD: Data,
    RhsG: Gradient + Overwrite,
    LhsD::Dim: Dimension + DimMax<RhsG::Dim>,
{
    pub fn new(left_data: Rc<LhsD>, right_data: Rc<RhsD>, right_grad: Rc<RhsG>) -> Self {
        let gradient = RefCell::new(broadcasted_zeros(&left_data.data(), &right_grad.gradient()));

        Self {
            left_data,
            right_data,
            right_grad,
            gradient,
            overwrite: Cell::new(true),
        }
    }
}

impl<LhsD, RhsD, RhsG> Backward for DivisionBackwardRight<LhsD, RhsD, RhsG>
where
    LhsD: Data,
    RhsD: Data,
    RhsG: Gradient + Overwrite,
    LhsD::Dim: Dimension + DimMax<RhsG::Dim>,
{
    fn backward(&self) {
        let mut rhs_grad = self.right_grad.gradient_mut();
        let grad = self.gradient.borrow();

        let mut tmp = Tensor::zeros(grad.raw_dim());
        Zip::from(&mut tmp)
            .and(&*grad)
            .and_broadcast(&*self.left_data.data())
            .and_broadcast(&*self.right_data.data())
            .par_for_each(|tmp_el, grad_el, lhs_data_el, rhs_data_el| {
                *tmp_el = -grad_el * lhs_data_el / rhs_data_el.powi(2)
            });

        let to_right_grad = reduce(&*rhs_grad, &tmp);
        if self.right_grad.can_overwrite() {
            Zip::from(&mut *rhs_grad)
                .and_broadcast(&to_right_grad.as_standard_layout())
                .par_for_each(|dest, src| *dest = *src);
            self.right_grad.set_overwrite(false);
        } else {
            Zip::from(&mut *rhs_grad)
                .and_broadcast(&to_right_grad.as_standard_layout())
                .par_for_each(|dest, src| *dest += *src);
        }
    }
}

impl<LhsD, RhsD, RhsG> Gradient for DivisionBackwardRight<LhsD, RhsD, RhsG>
where
    LhsD: Data,
    RhsD: Data,
    RhsG: Gradient + Overwrite,
    LhsD::Dim: Dimension + DimMax<RhsG::Dim>,
{
    type Dim = Broadcasted<LhsD::Dim, RhsG::Dim>;

    fn gradient(&self) -> Ref<Tensor<Self::Dim>> {
        self.gradient.borrow()
    }

    fn gradient_mut(&self) -> RefMut<Tensor<Self::Dim>> {
        self.gradient.borrow_mut()
    }
}

impl<LhsD, RhsD, RhsG> Overwrite for DivisionBackwardRight<LhsD, RhsD, RhsG>
where
    LhsD: Data,
    RhsD: Data,
    RhsG: Gradient + Overwrite,
    LhsD::Dim: Dimension + DimMax<RhsG::Dim>,
{
    fn can_overwrite(&self) -> bool {
        self.overwrite.get()
    }

    fn set_overwrite(&self, state: bool) {
        self.overwrite.set(state);
    }
}

// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~ MattrixMatrixMulBackward ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

pub struct MatrixMatrixMulBackward<LhsD, LhsG, RhsD, RhsG>
where
    LhsD: Data<Dim = Ix2>,
    RhsD: Data<Dim = Ix2>,
    LhsG: Gradient<Dim = Ix2> + Overwrite,
    RhsG: Gradient<Dim = Ix2> + Overwrite,
{
    left_data: Rc<LhsD>,
    left_grad: Rc<LhsG>,
    right_data: Rc<RhsD>,
    right_grad: Rc<RhsG>,
    gradient: RefCell<Tensor<Ix2>>,
    overwrite: Cell<bool>,
}

impl<LhsD, LhsG, RhsD, RhsG> MatrixMatrixMulBackward<LhsD, LhsG, RhsD, RhsG>
where
    LhsD: Data<Dim = Ix2>,
    RhsD: Data<Dim = Ix2>,
    LhsG: Gradient<Dim = Ix2> + Overwrite,
    RhsG: Gradient<Dim = Ix2> + Overwrite,
{
    pub fn new(
        left_data: Rc<LhsD>,
        left_grad: Rc<LhsG>,
        right_data: Rc<RhsD>,
        right_grad: Rc<RhsG>,
    ) -> Self {
        let shape = DotDim::shape(
            left_grad.gradient().raw_dim(),
            right_grad.gradient().raw_dim(),
        );
        let gradient = RefCell::new(Tensor::zeros((shape[0], shape[1])));

        Self {
            left_data,
            left_grad,
            right_data,
            right_grad,
            gradient,
            overwrite: Cell::new(true),
        }
    }
}

impl<LhsD, LhsG, RhsD, RhsG> Backward for MatrixMatrixMulBackward<LhsD, LhsG, RhsD, RhsG>
where
    LhsD: Data<Dim = Ix2>,
    RhsD: Data<Dim = Ix2>,
    LhsG: Gradient<Dim = Ix2> + Overwrite,
    RhsG: Gradient<Dim = Ix2> + Overwrite,
{
    fn backward(&self) {
        let (mut lhs_grad, lhs_data, mut rhs_grad, rhs_data) = {
            (
                self.left_grad.gradient_mut(),
                self.left_data.data(),
                self.right_grad.gradient_mut(),
                self.right_data.data(),
            )
        };
        let grad = self.gradient.borrow();

        if self.left_grad.can_overwrite() {
            general_mat_mul(1.0, &grad, &rhs_data.t(), 0., &mut lhs_grad);
            self.left_grad.set_overwrite(false);
        } else {
            general_mat_mul(1.0, &grad, &rhs_data.t(), 1., &mut lhs_grad);
        }

        if self.right_grad.can_overwrite() {
            general_mat_mul(1.0, &lhs_data.t(), &grad, 0., &mut rhs_grad);
            self.right_grad.set_overwrite(false);
        } else {
            general_mat_mul(1.0, &lhs_data.t(), &grad, 1., &mut rhs_grad);
        }
    }
}

impl<LhsD, LhsG, RhsD, RhsG> Gradient for MatrixMatrixMulBackward<LhsD, LhsG, RhsD, RhsG>
where
    LhsD: Data<Dim = Ix2>,
    RhsD: Data<Dim = Ix2>,
    LhsG: Gradient<Dim = Ix2> + Overwrite,
    RhsG: Gradient<Dim = Ix2> + Overwrite,
{
    type Dim = Ix2;

    fn gradient(&self) -> Ref<Tensor<Self::Dim>> {
        self.gradient.borrow()
    }

    fn gradient_mut(&self) -> RefMut<Tensor<Self::Dim>> {
        self.gradient.borrow_mut()
    }
}

impl<LhsD, LhsG, RhsD, RhsG> Overwrite for MatrixMatrixMulBackward<LhsD, LhsG, RhsD, RhsG>
where
    LhsD: Data<Dim = Ix2>,
    RhsD: Data<Dim = Ix2>,
    LhsG: Gradient<Dim = Ix2> + Overwrite,
    RhsG: Gradient<Dim = Ix2> + Overwrite,
{
    fn can_overwrite(&self) -> bool {
        self.overwrite.get()
    }

    fn set_overwrite(&self, state: bool) {
        self.overwrite.set(state);
    }
}

// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~ MatrixMatrixMulBackwardLeft ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

pub struct MatrixMatrixMulBackwardLeft<LhsG, RhsD>
where
    RhsD: Data<Dim = Ix2>,
    LhsG: Gradient<Dim = Ix2> + Overwrite,
{
    left_grad: Rc<LhsG>,
    right_data: Rc<RhsD>,
    gradient: RefCell<Tensor<Ix2>>,
    overwrite: Cell<bool>,
}

impl<LhsG, RhsD> MatrixMatrixMulBackwardLeft<LhsG, RhsD>
where
    RhsD: Data<Dim = Ix2>,
    LhsG: Gradient<Dim = Ix2> + Overwrite,
{
    pub fn new(left_grad: Rc<LhsG>, right_data: Rc<RhsD>) -> Self {
        let shape = DotDim::shape(left_grad.gradient().raw_dim(), right_data.data().raw_dim());
        let gradient = RefCell::new(Tensor::zeros((shape[0], shape[1])));

        Self {
            left_grad,
            right_data,
            gradient,
            overwrite: Cell::new(true),
        }
    }
}

impl<LhsG, RhsD> Backward for MatrixMatrixMulBackwardLeft<LhsG, RhsD>
where
    RhsD: Data<Dim = Ix2>,
    LhsG: Gradient<Dim = Ix2> + Overwrite,
{
    fn backward(&self) {
        let (mut lhs_grad, rhs_data) = { (self.left_grad.gradient_mut(), self.right_data.data()) };
        let grad = self.gradient.borrow();

        if self.left_grad.can_overwrite() {
            general_mat_mul(1.0, &grad, &rhs_data.t(), 0., &mut lhs_grad);
            self.left_grad.set_overwrite(false);
        } else {
            general_mat_mul(1.0, &grad, &rhs_data.t(), 1., &mut lhs_grad);
        }
    }
}

impl<LhsG, RhsD> Gradient for MatrixMatrixMulBackwardLeft<LhsG, RhsD>
where
    RhsD: Data<Dim = Ix2>,
    LhsG: Gradient<Dim = Ix2> + Overwrite,
{
    type Dim = Ix2;

    fn gradient(&self) -> Ref<Tensor<Self::Dim>> {
        self.gradient.borrow()
    }

    fn gradient_mut(&self) -> RefMut<Tensor<Self::Dim>> {
        self.gradient.borrow_mut()
    }
}

impl<LhsG, RhsD> Overwrite for MatrixMatrixMulBackwardLeft<LhsG, RhsD>
where
    RhsD: Data<Dim = Ix2>,
    LhsG: Gradient<Dim = Ix2> + Overwrite,
{
    fn can_overwrite(&self) -> bool {
        self.overwrite.get()
    }

    fn set_overwrite(&self, state: bool) {
        self.overwrite.set(state);
    }
}

// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~ MatrixMatrixMulBackwardRight ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

pub struct MatrixMatrixMulBackwardRight<LhsD, RhsG>
where
    LhsD: Data<Dim = Ix2>,
    RhsG: Gradient<Dim = Ix2> + Overwrite,
{
    left_data: Rc<LhsD>,
    right_grad: Rc<RhsG>,
    gradient: RefCell<Tensor<Ix2>>,
    overwrite: Cell<bool>,
}

impl<LhsD, RhsG> MatrixMatrixMulBackwardRight<LhsD, RhsG>
where
    LhsD: Data<Dim = Ix2>,
    RhsG: Gradient<Dim = Ix2> + Overwrite,
{
    pub fn new(left_data: Rc<LhsD>, right_grad: Rc<RhsG>) -> Self {
        let shape = DotDim::shape(left_data.data().raw_dim(), right_grad.gradient().raw_dim());
        let gradient = RefCell::new(Tensor::zeros((shape[0], shape[1])));

        Self {
            left_data,
            right_grad,
            gradient,
            overwrite: Cell::new(true),
        }
    }
}

impl<LhsD, RhsG> Backward for MatrixMatrixMulBackwardRight<LhsD, RhsG>
where
    LhsD: Data<Dim = Ix2>,
    RhsG: Gradient<Dim = Ix2> + Overwrite,
{
    fn backward(&self) {
        let (lhs_data, mut rhs_grad) = { (self.left_data.data(), self.right_grad.gradient_mut()) };
        let grad = self.gradient.borrow();

        if self.right_grad.can_overwrite() {
            general_mat_mul(1.0, &lhs_data.t(), &grad, 0., &mut rhs_grad);
            self.right_grad.set_overwrite(false);
        } else {
            general_mat_mul(1.0, &lhs_data.t(), &grad, 1., &mut rhs_grad);
        }
    }
}

impl<LhsD, RhsG> Gradient for MatrixMatrixMulBackwardRight<LhsD, RhsG>
where
    LhsD: Data<Dim = Ix2>,
    RhsG: Gradient<Dim = Ix2> + Overwrite,
{
    type Dim = Ix2;

    fn gradient(&self) -> Ref<Tensor<Self::Dim>> {
        self.gradient.borrow()
    }

    fn gradient_mut(&self) -> RefMut<Tensor<Self::Dim>> {
        self.gradient.borrow_mut()
    }
}

impl<LhsD, RhsG> Overwrite for MatrixMatrixMulBackwardRight<LhsD, RhsG>
where
    LhsD: Data<Dim = Ix2>,
    RhsG: Gradient<Dim = Ix2> + Overwrite,
{
    fn can_overwrite(&self) -> bool {
        self.overwrite.get()
    }

    fn set_overwrite(&self, state: bool) {
        self.overwrite.set(state);
    }
}

// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~ MatrixVectorMulBackward ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

pub struct MatrixVectorMulBackward<LhsD, LhsG, RhsD, RhsG>
where
    LhsD: Data<Dim = Ix2>,
    RhsD: Data<Dim = Ix1>,
    LhsG: Gradient<Dim = Ix2> + Overwrite,
    RhsG: Gradient<Dim = Ix1> + Overwrite,
{
    left_data: Rc<LhsD>,
    left_grad: Rc<LhsG>,
    right_data: Rc<RhsD>,
    right_grad: Rc<RhsG>,
    gradient: RefCell<Tensor<Ix1>>,
    overwrite: Cell<bool>,
}

impl<LhsD, LhsG, RhsD, RhsG> MatrixVectorMulBackward<LhsD, LhsG, RhsD, RhsG>
where
    LhsD: Data<Dim = Ix2>,
    RhsD: Data<Dim = Ix1>,
    LhsG: Gradient<Dim = Ix2> + Overwrite,
    RhsG: Gradient<Dim = Ix1> + Overwrite,
{
    pub fn new(
        left_data: Rc<LhsD>,
        left_grad: Rc<LhsG>,
        right_data: Rc<RhsD>,
        right_grad: Rc<RhsG>,
    ) -> Self {
        let shape = DotDim::shape(
            left_grad.gradient().raw_dim(),
            right_grad.gradient().raw_dim(),
        );
        let gradient = RefCell::new(Tensor::zeros(shape[0]));

        Self {
            left_data,
            left_grad,
            right_data,
            right_grad,
            gradient,
            overwrite: Cell::new(true),
        }
    }
}

impl<LhsD, LhsG, RhsD, RhsG> Backward for MatrixVectorMulBackward<LhsD, LhsG, RhsD, RhsG>
where
    LhsD: Data<Dim = Ix2>,
    RhsD: Data<Dim = Ix1>,
    LhsG: Gradient<Dim = Ix2> + Overwrite,
    RhsG: Gradient<Dim = Ix1> + Overwrite,
{
    fn backward(&self) {
        let (mut lhs_grad, lhs_data, mut rhs_grad, rhs_data) = {
            (
                self.left_grad.gradient_mut(),
                self.left_data.data(),
                self.right_grad.gradient_mut(),
                self.right_data.data(),
            )
        };
        let grad = self.gradient.borrow();
        let zip = Zip::from(&mut *lhs_grad)
            .and_broadcast(grad.slice(s![.., NewAxis]))
            .and_broadcast(&*rhs_data);

        if self.left_grad.can_overwrite() {
            zip.par_for_each(|lhs_grad_el, grad_el, rhs_data_el| {
                *lhs_grad_el = grad_el * rhs_data_el
            });
            self.left_grad.set_overwrite(false);
        } else {
            zip.par_for_each(|lhs_grad_el, grad_el, rhs_data_el| {
                *lhs_grad_el += grad_el * rhs_data_el
            });
        }

        if self.right_grad.can_overwrite() {
            general_mat_vec_mul(1.0, &lhs_data.t(), &grad, 0., &mut rhs_grad);
            self.right_grad.set_overwrite(false);
        } else {
            general_mat_vec_mul(1.0, &lhs_data.t(), &grad, 1., &mut rhs_grad);
        }
    }
}

impl<LhsD, LhsG, RhsD, RhsG> Gradient for MatrixVectorMulBackward<LhsD, LhsG, RhsD, RhsG>
where
    LhsD: Data<Dim = Ix2>,
    RhsD: Data<Dim = Ix1>,
    LhsG: Gradient<Dim = Ix2> + Overwrite,
    RhsG: Gradient<Dim = Ix1> + Overwrite,
{
    type Dim = Ix1;

    fn gradient(&self) -> Ref<Tensor<Self::Dim>> {
        self.gradient.borrow()
    }

    fn gradient_mut(&self) -> RefMut<Tensor<Self::Dim>> {
        self.gradient.borrow_mut()
    }
}

impl<LhsD, LhsG, RhsD, RhsG> Overwrite for MatrixVectorMulBackward<LhsD, LhsG, RhsD, RhsG>
where
    LhsD: Data<Dim = Ix2>,
    RhsD: Data<Dim = Ix1>,
    LhsG: Gradient<Dim = Ix2> + Overwrite,
    RhsG: Gradient<Dim = Ix1> + Overwrite,
{
    fn can_overwrite(&self) -> bool {
        self.overwrite.get()
    }

    fn set_overwrite(&self, state: bool) {
        self.overwrite.set(state);
    }
}

// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~ MatrixVectorMulBackwardLeft ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

pub struct MatrixVectorMulBackwardLeft<LhsG, RhsD>
where
    RhsD: Data<Dim = Ix1>,
    LhsG: Gradient<Dim = Ix2> + Overwrite,
{
    left_grad: Rc<LhsG>,
    right_data: Rc<RhsD>,
    gradient: RefCell<Tensor<Ix1>>,
    overwrite: Cell<bool>,
}

impl<LhsG, RhsD> MatrixVectorMulBackwardLeft<LhsG, RhsD>
where
    RhsD: Data<Dim = Ix1>,
    LhsG: Gradient<Dim = Ix2> + Overwrite,
{
    pub fn new(left_grad: Rc<LhsG>, right_data: Rc<RhsD>) -> Self {
        let shape = DotDim::shape(left_grad.gradient().raw_dim(), right_data.data().raw_dim());
        let gradient = RefCell::new(Tensor::zeros(shape[0]));

        Self {
            left_grad,
            right_data,
            gradient,
            overwrite: Cell::new(true),
        }
    }
}

impl<LhsG, RhsD> Backward for MatrixVectorMulBackwardLeft<LhsG, RhsD>
where
    RhsD: Data<Dim = Ix1>,
    LhsG: Gradient<Dim = Ix2> + Overwrite,
{
    fn backward(&self) {
        let (mut lhs_grad, rhs_data) = { (self.left_grad.gradient_mut(), self.right_data.data()) };
        let grad = self.gradient.borrow();
        let zip = Zip::from(&mut *lhs_grad)
            .and_broadcast(grad.slice(s![.., NewAxis]))
            .and_broadcast(&*rhs_data);

        if self.left_grad.can_overwrite() {
            zip.par_for_each(|lhs_grad_el, grad_el, rhs_data_el| {
                *lhs_grad_el = grad_el * rhs_data_el
            });
            self.left_grad.set_overwrite(false);
        } else {
            zip.par_for_each(|lhs_grad_el, grad_el, rhs_data_el| {
                *lhs_grad_el += grad_el * rhs_data_el
            });
        }
    }
}

impl<LhsG, RhsD> Gradient for MatrixVectorMulBackwardLeft<LhsG, RhsD>
where
    RhsD: Data<Dim = Ix1>,
    LhsG: Gradient<Dim = Ix2> + Overwrite,
{
    type Dim = Ix1;

    fn gradient(&self) -> Ref<Tensor<Self::Dim>> {
        self.gradient.borrow()
    }

    fn gradient_mut(&self) -> RefMut<Tensor<Self::Dim>> {
        self.gradient.borrow_mut()
    }
}

impl<LhsG, RhsD> Overwrite for MatrixVectorMulBackwardLeft<LhsG, RhsD>
where
    RhsD: Data<Dim = Ix1>,
    LhsG: Gradient<Dim = Ix2> + Overwrite,
{
    fn can_overwrite(&self) -> bool {
        self.overwrite.get()
    }

    fn set_overwrite(&self, state: bool) {
        self.overwrite.set(state);
    }
}

// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~ MatrixVectorMulBackwardRight ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

pub struct MatrixVectorMulBackwardRight<LhsD, RhsG>
where
    LhsD: Data<Dim = Ix2>,
    RhsG: Gradient<Dim = Ix1> + Overwrite,
{
    left_data: Rc<LhsD>,
    right_grad: Rc<RhsG>,
    gradient: RefCell<Tensor<Ix1>>,
    overwrite: Cell<bool>,
}

impl<LhsD, RhsG> MatrixVectorMulBackwardRight<LhsD, RhsG>
where
    LhsD: Data<Dim = Ix2>,
    RhsG: Gradient<Dim = Ix1> + Overwrite,
{
    pub fn new(left_data: Rc<LhsD>, right_grad: Rc<RhsG>) -> Self {
        let shape = DotDim::shape(left_data.data().raw_dim(), right_grad.gradient().raw_dim());
        let gradient = RefCell::new(Tensor::zeros(shape[0]));

        Self {
            left_data,
            right_grad,
            gradient,
            overwrite: Cell::new(true),
        }
    }
}

impl<LhsD, RhsG> Backward for MatrixVectorMulBackwardRight<LhsD, RhsG>
where
    LhsD: Data<Dim = Ix2>,
    RhsG: Gradient<Dim = Ix1> + Overwrite,
{
    fn backward(&self) {
        let (lhs_data, mut rhs_grad) = { (self.left_data.data(), self.right_grad.gradient_mut()) };

        let grad = self.gradient.borrow();
        if self.right_grad.can_overwrite() {
            general_mat_vec_mul(1.0, &lhs_data.t(), &grad, 0., &mut rhs_grad);
            self.right_grad.set_overwrite(false);
        } else {
            general_mat_vec_mul(1.0, &lhs_data.t(), &grad, 1., &mut rhs_grad);
        }
    }
}

impl<LhsD, RhsG> Gradient for MatrixVectorMulBackwardRight<LhsD, RhsG>
where
    LhsD: Data<Dim = Ix2>,
    RhsG: Gradient<Dim = Ix1> + Overwrite,
{
    type Dim = Ix1;

    fn gradient(&self) -> Ref<Tensor<Self::Dim>> {
        self.gradient.borrow()
    }

    fn gradient_mut(&self) -> RefMut<Tensor<Self::Dim>> {
        self.gradient.borrow_mut()
    }
}

impl<LhsD, RhsG> Overwrite for MatrixVectorMulBackwardRight<LhsD, RhsG>
where
    LhsD: Data<Dim = Ix2>,
    RhsG: Gradient<Dim = Ix1> + Overwrite,
{
    fn can_overwrite(&self) -> bool {
        self.overwrite.get()
    }

    fn set_overwrite(&self, state: bool) {
        self.overwrite.set(state);
    }
}

// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~ VectorMatrixMulBackward ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
pub struct VectorMatrixMulBackward<LhsD, LhsG, RhsD, RhsG>
where
    LhsD: Data<Dim = Ix1>,
    RhsD: Data<Dim = Ix2>,
    LhsG: Gradient<Dim = Ix1> + Overwrite,
    RhsG: Gradient<Dim = Ix2> + Overwrite,
{
    left_data: Rc<LhsD>,
    left_grad: Rc<LhsG>,
    right_data: Rc<RhsD>,
    right_grad: Rc<RhsG>,
    gradient: RefCell<Tensor<Ix1>>,
    overwrite: Cell<bool>,
}

impl<LhsD, LhsG, RhsD, RhsG> VectorMatrixMulBackward<LhsD, LhsG, RhsD, RhsG>
where
    LhsD: Data<Dim = Ix1>,
    RhsD: Data<Dim = Ix2>,
    LhsG: Gradient<Dim = Ix1> + Overwrite,
    RhsG: Gradient<Dim = Ix2> + Overwrite,
{
    pub fn new(
        left_data: Rc<LhsD>,
        left_grad: Rc<LhsG>,
        right_data: Rc<RhsD>,
        right_grad: Rc<RhsG>,
    ) -> Self {
        let shape = DotDim::shape(
            left_grad.gradient().raw_dim(),
            right_grad.gradient().raw_dim(),
        );
        let gradient = RefCell::new(Tensor::zeros(shape[0]));

        Self {
            left_data,
            left_grad,
            right_data,
            right_grad,
            gradient,
            overwrite: Cell::new(true),
        }
    }
}

impl<LhsD, LhsG, RhsD, RhsG> Backward for VectorMatrixMulBackward<LhsD, LhsG, RhsD, RhsG>
where
    LhsD: Data<Dim = Ix1>,
    RhsD: Data<Dim = Ix2>,
    LhsG: Gradient<Dim = Ix1> + Overwrite,
    RhsG: Gradient<Dim = Ix2> + Overwrite,
{
    fn backward(&self) {
        let (mut lhs_grad, lhs_data, mut rhs_grad, rhs_data) = {
            (
                self.left_grad.gradient_mut(),
                self.left_data.data(),
                self.right_grad.gradient_mut(),
                self.right_data.data(),
            )
        };
        let grad = self.gradient.borrow();

        if self.left_grad.can_overwrite() {
            general_mat_vec_mul(1.0, &rhs_data, &grad, 0., &mut lhs_grad);
            self.left_grad.set_overwrite(false);
        } else {
            general_mat_vec_mul(1.0, &rhs_data, &grad, 1., &mut lhs_grad);
        }

        let zip = Zip::from(&mut *rhs_grad)
            .and_broadcast(&*grad)
            .and_broadcast(lhs_data.slice(s![.., NewAxis]));

        if self.right_grad.can_overwrite() {
            zip.par_for_each(|rhs_grad_el, grad_el, lhs_data_el| {
                *rhs_grad_el = grad_el * lhs_data_el
            });
            self.right_grad.set_overwrite(false);
        } else {
            zip.par_for_each(|rhs_grad_el, grad_el, lhs_data_el| {
                *rhs_grad_el += grad_el * lhs_data_el
            });
        }
    }
}

impl<LhsD, LhsG, RhsD, RhsG> Gradient for VectorMatrixMulBackward<LhsD, LhsG, RhsD, RhsG>
where
    LhsD: Data<Dim = Ix1>,
    RhsD: Data<Dim = Ix2>,
    LhsG: Gradient<Dim = Ix1> + Overwrite,
    RhsG: Gradient<Dim = Ix2> + Overwrite,
{
    type Dim = Ix1;

    fn gradient(&self) -> Ref<Tensor<Self::Dim>> {
        self.gradient.borrow()
    }

    fn gradient_mut(&self) -> RefMut<Tensor<Self::Dim>> {
        self.gradient.borrow_mut()
    }
}

impl<LhsD, LhsG, RhsD, RhsG> Overwrite for VectorMatrixMulBackward<LhsD, LhsG, RhsD, RhsG>
where
    LhsD: Data<Dim = Ix1>,
    RhsD: Data<Dim = Ix2>,
    LhsG: Gradient<Dim = Ix1> + Overwrite,
    RhsG: Gradient<Dim = Ix2> + Overwrite,
{
    fn can_overwrite(&self) -> bool {
        self.overwrite.get()
    }

    fn set_overwrite(&self, state: bool) {
        self.overwrite.set(state);
    }
}

// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~ VectorMatrixMulBackwardLeft ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

pub struct VectorMatrixMulBackwardLeft<LhsG, RhsD>
where
    RhsD: Data<Dim = Ix2>,
    LhsG: Gradient<Dim = Ix1> + Overwrite,
{
    left_grad: Rc<LhsG>,
    right_data: Rc<RhsD>,
    gradient: RefCell<Tensor<Ix1>>,
    overwrite: Cell<bool>,
}

impl<LhsG, RhsD> VectorMatrixMulBackwardLeft<LhsG, RhsD>
where
    RhsD: Data<Dim = Ix2>,
    LhsG: Gradient<Dim = Ix1> + Overwrite,
{
    pub fn new(left_grad: Rc<LhsG>, right_data: Rc<RhsD>) -> Self {
        let shape = DotDim::shape(left_grad.gradient().raw_dim(), right_data.data().raw_dim());
        let gradient = RefCell::new(Tensor::zeros(shape[0]));

        Self {
            left_grad,
            right_data,
            gradient,
            overwrite: Cell::new(true),
        }
    }
}

impl<LhsG, RhsD> Backward for VectorMatrixMulBackwardLeft<LhsG, RhsD>
where
    RhsD: Data<Dim = Ix2>,
    LhsG: Gradient<Dim = Ix1> + Overwrite,
{
    fn backward(&self) {
        let (mut lhs_grad, rhs_data) = { (self.left_grad.gradient_mut(), self.right_data.data()) };
        let grad = self.gradient.borrow();

        if self.left_grad.can_overwrite() {
            general_mat_vec_mul(1.0, &rhs_data, &grad, 0., &mut lhs_grad);
            self.left_grad.set_overwrite(false);
        } else {
            general_mat_vec_mul(1.0, &rhs_data, &grad, 1., &mut lhs_grad);
        }
    }
}

impl<LhsG, RhsD> Gradient for VectorMatrixMulBackwardLeft<LhsG, RhsD>
where
    RhsD: Data<Dim = Ix2>,
    LhsG: Gradient<Dim = Ix1> + Overwrite,
{
    type Dim = Ix1;

    fn gradient(&self) -> Ref<Tensor<Self::Dim>> {
        self.gradient.borrow()
    }

    fn gradient_mut(&self) -> RefMut<Tensor<Self::Dim>> {
        self.gradient.borrow_mut()
    }
}

impl<LhsG, RhsD> Overwrite for VectorMatrixMulBackwardLeft<LhsG, RhsD>
where
    RhsD: Data<Dim = Ix2>,
    LhsG: Gradient<Dim = Ix1> + Overwrite,
{
    fn can_overwrite(&self) -> bool {
        self.overwrite.get()
    }

    fn set_overwrite(&self, state: bool) {
        self.overwrite.set(state);
    }
}

// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~ VectorMatrixMulBackwardRight ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

pub struct VectorMatrixMulBackwardRight<LhsD, RhsG>
where
    LhsD: Data<Dim = Ix1>,
    RhsG: Gradient<Dim = Ix2> + Overwrite,
{
    left_data: Rc<LhsD>,
    right_grad: Rc<RhsG>,
    gradient: RefCell<Tensor<Ix1>>,
    overwrite: Cell<bool>,
}

impl<LhsD, RhsG> VectorMatrixMulBackwardRight<LhsD, RhsG>
where
    LhsD: Data<Dim = Ix1>,
    RhsG: Gradient<Dim = Ix2> + Overwrite,
{
    pub fn new(left_data: Rc<LhsD>, right_grad: Rc<RhsG>) -> Self {
        let shape = DotDim::shape(left_data.data().raw_dim(), right_grad.gradient().raw_dim());
        let gradient = RefCell::new(Tensor::zeros(shape[0]));

        Self {
            left_data,
            right_grad,
            gradient,
            overwrite: Cell::new(true),
        }
    }
}

impl<LhsD, RhsG> Backward for VectorMatrixMulBackwardRight<LhsD, RhsG>
where
    LhsD: Data<Dim = Ix1>,
    RhsG: Gradient<Dim = Ix2> + Overwrite,
{
    fn backward(&self) {
        let (lhs_data, mut rhs_grad) = { (self.left_data.data(), self.right_grad.gradient_mut()) };
        let grad = self.gradient.borrow();

        let zip = Zip::from(&mut *rhs_grad)
            .and_broadcast(&*grad)
            .and_broadcast(lhs_data.slice(s![.., NewAxis]));

        if self.right_grad.can_overwrite() {
            zip.par_for_each(|rhs_grad_el, grad_el, lhs_data_el| {
                *rhs_grad_el = grad_el * lhs_data_el
            });
            self.right_grad.set_overwrite(false);
        } else {
            zip.par_for_each(|rhs_grad_el, grad_el, lhs_data_el| {
                *rhs_grad_el += grad_el * lhs_data_el
            });
        }
    }
}

impl<LhsD, RhsG> Gradient for VectorMatrixMulBackwardRight<LhsD, RhsG>
where
    LhsD: Data<Dim = Ix1>,
    RhsG: Gradient<Dim = Ix2> + Overwrite,
{
    type Dim = Ix1;

    fn gradient(&self) -> Ref<Tensor<Self::Dim>> {
        self.gradient.borrow()
    }

    fn gradient_mut(&self) -> RefMut<Tensor<Self::Dim>> {
        self.gradient.borrow_mut()
    }
}

impl<LhsD, RhsG> Overwrite for VectorMatrixMulBackwardRight<LhsD, RhsG>
where
    LhsD: Data<Dim = Ix1>,
    RhsG: Gradient<Dim = Ix2> + Overwrite,
{
    fn can_overwrite(&self) -> bool {
        self.overwrite.get()
    }

    fn set_overwrite(&self, state: bool) {
        self.overwrite.set(state);
    }
}

// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~ VectorVectorMulBackward ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

pub struct VectorVectorMulBackward<LhsD, LhsG, RhsD, RhsG>
where
    LhsD: Data<Dim = Ix1>,
    RhsD: Data<Dim = Ix1>,
    LhsG: Gradient<Dim = Ix1> + Overwrite,
    RhsG: Gradient<Dim = Ix1> + Overwrite,
{
    left_data: Rc<LhsD>,
    left_grad: Rc<LhsG>,
    right_data: Rc<RhsD>,
    right_grad: Rc<RhsG>,
    gradient: RefCell<Tensor<Ix1>>,
    overwrite: Cell<bool>,
}

impl<LhsD, LhsG, RhsD, RhsG> VectorVectorMulBackward<LhsD, LhsG, RhsD, RhsG>
where
    LhsD: Data<Dim = Ix1>,
    RhsD: Data<Dim = Ix1>,
    LhsG: Gradient<Dim = Ix1> + Overwrite,
    RhsG: Gradient<Dim = Ix1> + Overwrite,
{
    pub fn new(
        left_data: Rc<LhsD>,
        left_grad: Rc<LhsG>,
        right_data: Rc<RhsD>,
        right_grad: Rc<RhsG>,
    ) -> Self {
        let shape = DotDim::shape(
            left_grad.gradient().raw_dim(),
            right_grad.gradient().raw_dim(),
        );
        let gradient = RefCell::new(Tensor::zeros(shape[0]));

        Self {
            left_data,
            left_grad,
            right_data,
            right_grad,
            gradient,
            overwrite: Cell::new(true),
        }
    }
}

impl<LhsD, LhsG, RhsD, RhsG> Backward for VectorVectorMulBackward<LhsD, LhsG, RhsD, RhsG>
where
    LhsD: Data<Dim = Ix1>,
    RhsD: Data<Dim = Ix1>,
    LhsG: Gradient<Dim = Ix1> + Overwrite,
    RhsG: Gradient<Dim = Ix1> + Overwrite,
{
    fn backward(&self) {
        let (mut lhs_grad, lhs_data, mut rhs_grad, rhs_data) = {
            (
                self.left_grad.gradient_mut(),
                self.left_data.data(),
                self.right_grad.gradient_mut(),
                self.right_data.data(),
            )
        };
        let grad = self.gradient.borrow();

        let left_zip = Zip::from(&mut *lhs_grad).and(&*rhs_data);
        if self.left_grad.can_overwrite() {
            left_zip.for_each(|lhs_grad_el, rhs_data_el| *lhs_grad_el = rhs_data_el * grad[0]);
            self.left_grad.set_overwrite(false);
        } else {
            left_zip.for_each(|lhs_grad_el, rhs_data_el| *lhs_grad_el += rhs_data_el * grad[0]);
        }

        let right_zip = Zip::from(&mut *rhs_grad).and(&*lhs_data);
        if self.right_grad.can_overwrite() {
            right_zip.for_each(|rhs_grad_el, lhs_data_el| *rhs_grad_el = lhs_data_el * grad[0]);
            self.right_grad.set_overwrite(false);
        } else {
            right_zip.for_each(|rhs_grad_el, lhs_data_el| *rhs_grad_el += lhs_data_el * grad[0]);
        }
    }
}

impl<LhsD, LhsG, RhsD, RhsG> Gradient for VectorVectorMulBackward<LhsD, LhsG, RhsD, RhsG>
where
    LhsD: Data<Dim = Ix1>,
    RhsD: Data<Dim = Ix1>,
    LhsG: Gradient<Dim = Ix1> + Overwrite,
    RhsG: Gradient<Dim = Ix1> + Overwrite,
{
    type Dim = Ix1;

    fn gradient(&self) -> Ref<Tensor<Self::Dim>> {
        self.gradient.borrow()
    }

    fn gradient_mut(&self) -> RefMut<Tensor<Self::Dim>> {
        self.gradient.borrow_mut()
    }
}

impl<LhsD, LhsG, RhsD, RhsG> Overwrite for VectorVectorMulBackward<LhsD, LhsG, RhsD, RhsG>
where
    LhsD: Data<Dim = Ix1>,
    RhsD: Data<Dim = Ix1>,
    LhsG: Gradient<Dim = Ix1> + Overwrite,
    RhsG: Gradient<Dim = Ix1> + Overwrite,
{
    fn can_overwrite(&self) -> bool {
        self.overwrite.get()
    }

    fn set_overwrite(&self, state: bool) {
        self.overwrite.set(state);
    }
}

// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~ VectorVectorMulBackwardUnary ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

pub struct VectorVectorMulBackwardUnary<T, U>
where
    T: Gradient<Dim = Ix1> + Overwrite,
    U: Data<Dim = Ix1>,
{
    diff_operand: Rc<T>,
    no_diff_operand: Rc<U>,
    gradient: RefCell<Tensor<Ix1>>,
    overwrite: Cell<bool>,
}

impl<T, U> VectorVectorMulBackwardUnary<T, U>
where
    T: Gradient<Dim = Ix1> + Overwrite,
    U: Data<Dim = Ix1>,
{
    pub fn new(diff_operand: Rc<T>, no_diff_operand: Rc<U>) -> Self {
        let shape = DotDim::shape(
            diff_operand.gradient().raw_dim(),
            no_diff_operand.data().raw_dim(),
        );
        let gradient = RefCell::new(Tensor::zeros(shape[0]));

        Self {
            diff_operand,
            no_diff_operand,
            gradient,
            overwrite: Cell::new(true),
        }
    }
}

impl<T, U> Backward for VectorVectorMulBackwardUnary<T, U>
where
    T: Gradient<Dim = Ix1> + Overwrite,
    U: Data<Dim = Ix1>,
{
    fn backward(&self) {
        let (mut diff_op_grad, no_diff_op_data) = {
            (
                self.diff_operand.gradient_mut(),
                self.no_diff_operand.data(),
            )
        };
        let grad = self.gradient.borrow();

        let zip = Zip::from(&mut *diff_op_grad).and(&*no_diff_op_data);
        if self.diff_operand.can_overwrite() {
            zip.for_each(|diff_op_grad_el, no_diff_op_data_el| {
                *diff_op_grad_el = no_diff_op_data_el * grad[0]
            });
            self.diff_operand.set_overwrite(false);
        } else {
            zip.for_each(|diff_op_grad_el, no_diff_op_data_el| {
                *diff_op_grad_el += no_diff_op_data_el * grad[0]
            });
        }
    }
}

impl<T, U> Gradient for VectorVectorMulBackwardUnary<T, U>
where
    T: Gradient<Dim = Ix1> + Overwrite,
    U: Data<Dim = Ix1>,
{
    type Dim = Ix1;

    fn gradient(&self) -> Ref<Tensor<Self::Dim>> {
        self.gradient.borrow()
    }

    fn gradient_mut(&self) -> RefMut<Tensor<Self::Dim>> {
        self.gradient.borrow_mut()
    }
}

impl<T, U> Overwrite for VectorVectorMulBackwardUnary<T, U>
where
    T: Gradient<Dim = Ix1> + Overwrite,
    U: Data<Dim = Ix1>,
{
    fn can_overwrite(&self) -> bool {
        self.overwrite.get()
    }

    fn set_overwrite(&self, state: bool) {
        self.overwrite.set(state);
    }
}

// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~ PowerBackward ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

pub struct PowerBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    operand_grad: Rc<T>,
    operand_data: Rc<U>,
    exp: i32,
    gradient: RefCell<Tensor<T::Dim>>,
    overwrite: Cell<bool>,
}

impl<T, U> PowerBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    pub fn new(operand_grad: Rc<T>, operand_data: Rc<U>, exp: i32) -> Self {
        let gradient = RefCell::new(Tensor::zeros(operand_grad.gradient().raw_dim()));

        Self {
            operand_grad,
            operand_data,
            exp,
            gradient,
            overwrite: Cell::new(true),
        }
    }
}

impl<T, U> Gradient for PowerBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    type Dim = T::Dim;

    fn gradient(&self) -> Ref<Tensor<Self::Dim>> {
        self.gradient.borrow()
    }

    fn gradient_mut(&self) -> RefMut<Tensor<Self::Dim>> {
        self.gradient.borrow_mut()
    }
}

impl<T, U> Backward for PowerBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    fn backward(&self) {
        let mut op_grad = self.operand_grad.gradient_mut();
        let op_data = self.operand_data.data();
        let grad = self.gradient.borrow();
        let exp = self.exp;

        let zip = Zip::from(&mut *op_grad).and(&*grad).and(&*op_data);
        if self.operand_grad.can_overwrite() {
            zip.par_for_each(|op_grad_el, grad_el, op_data_el| {
                *op_grad_el = grad_el * op_data_el.powi(exp - 1) * exp as f32
            });
            self.operand_grad.set_overwrite(false);
        } else {
            zip.par_for_each(|op_grad_el, grad_el, op_data_el| {
                *op_grad_el += grad_el * op_data_el.powi(exp - 1) * exp as f32
            });
        }
    }
}

impl<T, U> Overwrite for PowerBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    fn can_overwrite(&self) -> bool {
        self.overwrite.get()
    }

    fn set_overwrite(&self, state: bool) {
        self.overwrite.set(state);
    }
}

// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~ SumBackward ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

pub struct SumBackward<T: Gradient + Overwrite> {
    operand: Rc<T>,
    gradient: RefCell<Tensor<Ix1>>,
    overwrite: Cell<bool>,
}

impl<T: Gradient + Overwrite> SumBackward<T> {
    pub fn new(operand: Rc<T>) -> Self {
        let gradient = RefCell::new(Tensor::zeros(1));

        Self {
            operand,
            gradient,
            overwrite: Cell::new(true),
        }
    }
}

impl<T: Gradient + Overwrite> Gradient for SumBackward<T> {
    type Dim = Ix1;

    fn gradient(&self) -> Ref<Tensor<Self::Dim>> {
        self.gradient.borrow()
    }

    fn gradient_mut(&self) -> RefMut<Tensor<Self::Dim>> {
        self.gradient.borrow_mut()
    }
}

impl<T: Gradient + Overwrite> Backward for SumBackward<T> {
    fn backward(&self) {
        let mut op_grad = self.operand.gradient_mut();
        let grad = self.gradient.borrow();

        let zip = Zip::from(&mut *op_grad).and_broadcast(&*grad);
        if self.operand.can_overwrite() {
            zip.par_for_each(|op_grad_el, grad_el| *op_grad_el = *grad_el);
            self.operand.set_overwrite(false);
        } else {
            zip.par_for_each(|op_grad_el, grad_el| *op_grad_el += *grad_el);
        }
    }
}

impl<T: Gradient + Overwrite> Overwrite for SumBackward<T> {
    fn can_overwrite(&self) -> bool {
        self.overwrite.get()
    }

    fn set_overwrite(&self, state: bool) {
        self.overwrite.set(state);
    }
}

// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~ LognBackward ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

pub struct LognBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    operand_grad: Rc<T>,
    operand_data: Rc<U>,
    gradient: RefCell<Tensor<T::Dim>>,
    overwrite: Cell<bool>,
}

impl<T, U> LognBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    pub fn new(operand_grad: Rc<T>, operand_data: Rc<U>) -> Self {
        let gradient = RefCell::new(Tensor::zeros(operand_grad.gradient().raw_dim()));

        Self {
            operand_grad,
            operand_data,
            gradient,
            overwrite: Cell::new(true),
        }
    }
}

impl<T, U> Gradient for LognBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    type Dim = T::Dim;

    fn gradient(&self) -> Ref<Tensor<Self::Dim>> {
        self.gradient.borrow()
    }

    fn gradient_mut(&self) -> RefMut<Tensor<Self::Dim>> {
        self.gradient.borrow_mut()
    }
}

impl<T, U> Backward for LognBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    fn backward(&self) {
        let mut op_grad = self.operand_grad.gradient_mut();
        let op_data = self.operand_data.data();
        let grad = self.gradient.borrow();

        let zip = Zip::from(&mut *op_grad).and(&*grad).and(&*op_data);
        if self.operand_grad.can_overwrite() {
            zip.par_for_each(|op_grad_el, grad_el, op_data_el| *op_grad_el = grad_el / op_data_el);
            self.operand_grad.set_overwrite(false);
        } else {
            zip.par_for_each(|op_grad_el, grad_el, op_data_el| *op_grad_el += grad_el / op_data_el);
        }
    }
}

impl<T, U> Overwrite for LognBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    fn can_overwrite(&self) -> bool {
        self.overwrite.get()
    }

    fn set_overwrite(&self, state: bool) {
        self.overwrite.set(state);
    }
}

// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~ ReLUBackward ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

#[allow(clippy::clippy::upper_case_acronyms)]
pub struct ReLUBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    operand_grad: Rc<T>,
    operand_data: Rc<U>,
    gradient: RefCell<Tensor<T::Dim>>,
    overwrite: Cell<bool>,
}

impl<T, U> ReLUBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    pub fn new(operand_grad: Rc<T>, operand_data: Rc<U>) -> Self {
        let gradient = RefCell::new(Tensor::zeros(operand_grad.gradient().raw_dim()));

        Self {
            operand_grad,
            operand_data,
            gradient,
            overwrite: Cell::new(true),
        }
    }
}

impl<T, U> Gradient for ReLUBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    type Dim = T::Dim;

    fn gradient(&self) -> Ref<Tensor<Self::Dim>> {
        self.gradient.borrow()
    }

    fn gradient_mut(&self) -> RefMut<Tensor<Self::Dim>> {
        self.gradient.borrow_mut()
    }
}

impl<T, U> Backward for ReLUBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    fn backward(&self) {
        let mut op_grad = self.operand_grad.gradient_mut();
        let op_data = self.operand_data.data();
        let grad = self.gradient.borrow();

        let zip = Zip::from(&mut *op_grad).and(&*grad).and(&*op_data);
        if self.operand_grad.can_overwrite() {
            zip.par_for_each(|op_grad_el, grad_el, op_data_el| {
                *op_grad_el = if *op_data_el > 0.0 { *grad_el } else { 0.0 }
            });
            self.operand_grad.set_overwrite(false);
        } else {
            zip.par_for_each(|op_grad_el, grad_el, op_data_el| {
                *op_grad_el += if *op_data_el > 0.0 { *grad_el } else { 0.0 }
            });
        }
    }
}

impl<T, U> Overwrite for ReLUBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    fn can_overwrite(&self) -> bool {
        self.overwrite.get()
    }

    fn set_overwrite(&self, state: bool) {
        self.overwrite.set(state);
    }
}

// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~ LeakyReLUBackward ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

#[allow(clippy::clippy::upper_case_acronyms)]
pub struct LeakyReLUBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    operand_grad: Rc<T>,
    operand_data: Rc<U>,
    gradient: RefCell<Tensor<T::Dim>>,
    overwrite: Cell<bool>,
}

impl<T, U> LeakyReLUBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    pub fn new(operand_grad: Rc<T>, operand_data: Rc<U>) -> Self {
        let gradient = RefCell::new(Tensor::zeros(operand_grad.gradient().raw_dim()));

        Self {
            operand_grad,
            operand_data,
            gradient,
            overwrite: Cell::new(true),
        }
    }
}

impl<T, U> Gradient for LeakyReLUBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    type Dim = T::Dim;

    fn gradient(&self) -> Ref<Tensor<Self::Dim>> {
        self.gradient.borrow()
    }

    fn gradient_mut(&self) -> RefMut<Tensor<Self::Dim>> {
        self.gradient.borrow_mut()
    }
}

impl<T, U> Backward for LeakyReLUBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    fn backward(&self) {
        let mut op_grad = self.operand_grad.gradient_mut();
        let op_data = self.operand_data.data();
        let grad = self.gradient.borrow();

        let zip = Zip::from(&mut *op_grad).and(&*grad).and(&*op_data);
        if self.operand_grad.can_overwrite() {
            zip.par_for_each(|op_grad_el, grad_el, op_data_el| {
                *op_grad_el = if *op_data_el > 0.0 { *grad_el } else { 0.01 }
            });
            self.operand_grad.set_overwrite(false);
        } else {
            zip.par_for_each(|op_grad_el, grad_el, op_data_el| {
                *op_grad_el += if *op_data_el > 0.0 { *grad_el } else { 0.01 }
            });
        }
    }
}

impl<T, U> Overwrite for LeakyReLUBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    fn can_overwrite(&self) -> bool {
        self.overwrite.get()
    }

    fn set_overwrite(&self, state: bool) {
        self.overwrite.set(state);
    }
}

// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~ SoftPlusBackward ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~S

pub struct SoftPlusBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    operand_grad: Rc<T>,
    operand_data: Rc<U>,
    gradient: RefCell<Tensor<T::Dim>>,
    overwrite: Cell<bool>,
}

impl<T, U> SoftPlusBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    pub fn new(operand_grad: Rc<T>, operand_data: Rc<U>) -> Self {
        let gradient = RefCell::new(Tensor::zeros(operand_grad.gradient().raw_dim()));

        Self {
            operand_grad,
            operand_data,
            gradient,
            overwrite: Cell::new(true),
        }
    }
}

impl<T, U> Gradient for SoftPlusBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    type Dim = T::Dim;

    fn gradient(&self) -> Ref<Tensor<Self::Dim>> {
        self.gradient.borrow()
    }

    fn gradient_mut(&self) -> RefMut<Tensor<Self::Dim>> {
        self.gradient.borrow_mut()
    }
}

impl<T, U> Backward for SoftPlusBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    fn backward(&self) {
        let mut op_grad = self.operand_grad.gradient_mut();
        let op_data = self.operand_data.data();
        let grad = self.gradient.borrow();

        let zip = Zip::from(&mut *op_grad).and(&*grad).and(&*op_data);
        if self.operand_grad.can_overwrite() {
            zip.par_for_each(|op_grad_el, grad_el, op_data_el| {
                *op_grad_el = if *op_data_el >= 15.0 {
                    *grad_el
                } else if *op_data_el <= -15.0 {
                    0.0
                } else {
                    grad_el / (1.0 + (-*op_data_el).exp())
                }
            });
            self.operand_grad.set_overwrite(false);
        } else {
            zip.par_for_each(|op_grad_el, grad_el, op_data_el| {
                *op_grad_el += if *op_data_el >= 15.0 {
                    *grad_el
                } else if *op_data_el <= -15.0 {
                    0.0
                } else {
                    grad_el / (1.0 + (-*op_data_el).exp())
                }
            });
        }
    }
}

impl<T, U> Overwrite for SoftPlusBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    fn can_overwrite(&self) -> bool {
        self.overwrite.get()
    }

    fn set_overwrite(&self, state: bool) {
        self.overwrite.set(state);
    }
}

// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~ SigmoidBackward ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

pub struct SigmoidBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    operand_grad: Rc<T>,
    data: Rc<U>,
    gradient: RefCell<Tensor<T::Dim>>,
    overwrite: Cell<bool>,
}

impl<T, U> SigmoidBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    pub fn new(operand_grad: Rc<T>, data: Rc<U>) -> Self {
        let gradient = RefCell::new(Tensor::zeros(operand_grad.gradient().raw_dim()));

        Self {
            operand_grad,
            data,
            gradient,
            overwrite: Cell::new(true),
        }
    }
}

impl<T, U> Gradient for SigmoidBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    type Dim = T::Dim;

    fn gradient(&self) -> Ref<Tensor<Self::Dim>> {
        self.gradient.borrow()
    }

    fn gradient_mut(&self) -> RefMut<Tensor<Self::Dim>> {
        self.gradient.borrow_mut()
    }
}

impl<T, U> Backward for SigmoidBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    fn backward(&self) {
        let mut op_grad = self.operand_grad.gradient_mut();
        let data = self.data.data();
        let grad = self.gradient.borrow();

        let zip = Zip::from(&mut *op_grad).and(&*grad).and(&*data);
        if self.operand_grad.can_overwrite() {
            zip.par_for_each(|op_grad_el, grad_el, data_el| {
                *op_grad_el = *grad_el * *data_el * (1.0 - *data_el)
            });
            self.operand_grad.set_overwrite(false);
        } else {
            zip.par_for_each(|op_grad_el, grad_el, data_el| {
                *op_grad_el += *grad_el * *data_el * (1.0 - *data_el)
            });
        }
    }
}

impl<T, U> Overwrite for SigmoidBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    fn can_overwrite(&self) -> bool {
        self.overwrite.get()
    }

    fn set_overwrite(&self, state: bool) {
        self.overwrite.set(state);
    }
}

// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~ TanHBackward ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

pub struct TanHBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    operand_grad: Rc<T>,
    data: Rc<U>,
    gradient: RefCell<Tensor<T::Dim>>,
    overwrite: Cell<bool>,
}

impl<T, U> TanHBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    pub fn new(operand_grad: Rc<T>, data: Rc<U>) -> Self {
        let gradient = RefCell::new(Tensor::zeros(operand_grad.gradient().raw_dim()));

        Self {
            operand_grad,
            data,
            gradient,
            overwrite: Cell::new(true),
        }
    }
}

impl<T, U> Gradient for TanHBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    type Dim = T::Dim;

    fn gradient(&self) -> Ref<Tensor<Self::Dim>> {
        self.gradient.borrow()
    }

    fn gradient_mut(&self) -> RefMut<Tensor<Self::Dim>> {
        self.gradient.borrow_mut()
    }
}

impl<T, U> Backward for TanHBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    fn backward(&self) {
        let mut op_grad = self.operand_grad.gradient_mut();
        let data = self.data.data();
        let grad = self.gradient.borrow();

        let zip = Zip::from(&mut *op_grad).and(&*grad).and(&*data);
        if self.operand_grad.can_overwrite() {
            zip.par_for_each(|op_grad_el, grad_el, data_el| {
                *op_grad_el = *grad_el * (1.0 - data_el.powi(2))
            });
            self.operand_grad.set_overwrite(false);
        } else {
            zip.par_for_each(|op_grad_el, grad_el, data_el| {
                *op_grad_el += *grad_el * (1.0 - data_el.powi(2))
            });
        }
    }
}

impl<T, U> Overwrite for TanHBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    fn can_overwrite(&self) -> bool {
        self.overwrite.get()
    }

    fn set_overwrite(&self, state: bool) {
        self.overwrite.set(state);
    }
}

// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~ ExpBackward ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

pub struct ExpBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    operand_grad: Rc<T>,
    data: Rc<U>,
    gradient: RefCell<Tensor<T::Dim>>,
    overwrite: Cell<bool>,
}

impl<T, U> ExpBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    pub fn new(operand_grad: Rc<T>, data: Rc<U>) -> Self {
        let gradient = RefCell::new(Tensor::zeros(operand_grad.gradient().raw_dim()));

        Self {
            operand_grad,
            data,
            gradient,
            overwrite: Cell::new(true),
        }
    }
}

impl<T, U> Gradient for ExpBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    type Dim = T::Dim;

    fn gradient(&self) -> Ref<Tensor<Self::Dim>> {
        self.gradient.borrow()
    }

    fn gradient_mut(&self) -> RefMut<Tensor<Self::Dim>> {
        self.gradient.borrow_mut()
    }
}

impl<T, U> Backward for ExpBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    fn backward(&self) {
        let mut op_grad = self.operand_grad.gradient_mut();
        let data = self.data.data();
        let grad = self.gradient.borrow();

        let zip = Zip::from(&mut *op_grad).and(&*grad).and(&*data);
        if self.operand_grad.can_overwrite() {
            zip.par_for_each(|op_grad_el, grad_el, data_el| *op_grad_el = *grad_el * data_el);
            self.operand_grad.set_overwrite(false);
        } else {
            zip.par_for_each(|op_grad_el, grad_el, data_el| *op_grad_el += *grad_el * data_el);
        }
    }
}

impl<T, U> Overwrite for ExpBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    fn can_overwrite(&self) -> bool {
        self.overwrite.get()
    }

    fn set_overwrite(&self, state: bool) {
        self.overwrite.set(state);
    }
}

// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~ SoftmaxBackward ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

pub struct SoftmaxBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    operand_grad: Rc<T>,
    data: Rc<U>,
    axis: usize,
    gradient: RefCell<Tensor<T::Dim>>,
    overwrite: Cell<bool>,
}

impl<T, U> SoftmaxBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    pub fn new(operand_grad: Rc<T>, data: Rc<U>, axis: usize) -> Self {
        let gradient = RefCell::new(Tensor::zeros(operand_grad.gradient().raw_dim()));

        Self {
            operand_grad,
            data,
            axis,
            gradient,
            overwrite: Cell::new(true),
        }
    }
}

impl<T, U> Gradient for SoftmaxBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    type Dim = T::Dim;

    fn gradient(&self) -> Ref<Tensor<Self::Dim>> {
        self.gradient.borrow()
    }

    fn gradient_mut(&self) -> RefMut<Tensor<Self::Dim>> {
        self.gradient.borrow_mut()
    }
}

impl<T, U> Backward for SoftmaxBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    fn backward(&self) {
        let mut op_grad = self.operand_grad.gradient_mut();
        let data = self.data.data();
        let grad = self.gradient.borrow();
        let axis = self.axis;
        let zip = Zip::from(op_grad.lanes_mut(Axis(axis)))
            .and(grad.lanes(Axis(axis)))
            .and(data.lanes(Axis(axis)));

        if self.operand_grad.can_overwrite() {
            zip.par_for_each(|mut op_grad_lane, grad_lane, data_lane| {
                let sum = Zip::from(grad_lane)
                    .and(data_lane)
                    .fold(0., |acc, grad_el, data_el| acc + grad_el * data_el);
                Zip::from(&mut op_grad_lane)
                    .and(&grad_lane)
                    .and(&data_lane)
                    .for_each(|op_grad_el, grad_el, data_el| {
                        *op_grad_el = data_el * (grad_el - sum)
                    })
            });
            self.operand_grad.set_overwrite(false);
        } else {
            zip.par_for_each(|mut op_grad_lane, grad_lane, data_lane| {
                let sum = Zip::from(grad_lane)
                    .and(data_lane)
                    .fold(0., |acc, grad_el, data_el| acc + grad_el * data_el);
                Zip::from(&mut op_grad_lane)
                    .and(&grad_lane)
                    .and(&data_lane)
                    .for_each(|op_grad_el, grad_el, data_el| {
                        *op_grad_el += data_el * (grad_el - sum)
                    })
            });
        }
    }
}

impl<T, U> Overwrite for SoftmaxBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    fn can_overwrite(&self) -> bool {
        self.overwrite.get()
    }

    fn set_overwrite(&self, state: bool) {
        self.overwrite.set(state);
    }
}

// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~ LogSoftmaxBackward ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

pub struct LogSoftmaxBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    operand_grad: Rc<T>,
    forward_data: Rc<U>,
    axis: usize,
    gradient: RefCell<Tensor<T::Dim>>,
    overwrite: Cell<bool>,
}

impl<T, U> LogSoftmaxBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    pub fn new(operand_grad: Rc<T>, forward_data: Rc<U>, axis: usize) -> Self {
        let gradient = RefCell::new(Tensor::zeros(operand_grad.gradient().raw_dim()));

        Self {
            operand_grad,
            forward_data,
            axis,
            gradient,
            overwrite: Cell::new(true),
        }
    }
}

impl<T, U> Gradient for LogSoftmaxBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    type Dim = T::Dim;

    fn gradient(&self) -> Ref<Tensor<Self::Dim>> {
        self.gradient.borrow()
    }

    fn gradient_mut(&self) -> RefMut<Tensor<Self::Dim>> {
        self.gradient.borrow_mut()
    }
}

impl<T, U> Backward for LogSoftmaxBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    fn backward(&self) {
        let mut op_grad = self.operand_grad.gradient_mut();
        let data = self.forward_data.data();
        let grad = self.gradient.borrow();
        let axis = self.axis;

        let zip = Zip::from(op_grad.lanes_mut(Axis(axis)))
            .and(grad.lanes(Axis(axis)))
            .and(data.lanes(Axis(axis)));
        if self.operand_grad.can_overwrite() {
            zip.par_for_each(|mut op_grad_lane, grad_lane, data_lane| {
                let gradient_sum = grad_lane.sum();
                Zip::from(&mut op_grad_lane)
                    .and(&grad_lane)
                    .and(&data_lane)
                    .for_each(|op_grad_el, grad_el, data_el| {
                        *op_grad_el = grad_el - data_el.exp() * gradient_sum
                    })
            });
            self.operand_grad.set_overwrite(false);
        } else {
            zip.par_for_each(|mut op_grad_lane, grad_lane, data_lane| {
                let gradient_sum = grad_lane.sum();
                Zip::from(&mut op_grad_lane)
                    .and(&grad_lane)
                    .and(&data_lane)
                    .for_each(|op_grad_el, grad_el, data_el| {
                        *op_grad_el += grad_el - data_el.exp() * gradient_sum
                    })
            });
        }
    }
}

impl<T, U> Overwrite for LogSoftmaxBackward<T, U>
where
    T: Gradient + Overwrite,
    U: Data<Dim = T::Dim>,
{
    fn can_overwrite(&self) -> bool {
        self.overwrite.get()
    }

    fn set_overwrite(&self, state: bool) {
        self.overwrite.set(state);
    }
}

// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~ ConcatenateBackward ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

pub struct ConcatenateBackward<Lhs, Rhs>
where
    Lhs: Gradient + Overwrite,
    Rhs: Gradient<Dim = Lhs::Dim> + Overwrite,
    Lhs::Dim: RemoveAxis,
{
    left: Rc<Lhs>,
    right: Rc<Rhs>,
    axis: usize,
    gradient: RefCell<Tensor<Lhs::Dim>>,
    overwrite: Cell<bool>,
}

impl<Lhs, Rhs> ConcatenateBackward<Lhs, Rhs>
where
    Lhs: Gradient + Overwrite,
    Rhs: Gradient<Dim = Lhs::Dim> + Overwrite,
    Lhs::Dim: RemoveAxis,
{
    pub fn new(left: Rc<Lhs>, right: Rc<Rhs>, axis: usize) -> Self {
        let gradient = RefCell::new(
            concatenate(
                Axis(axis),
                &[left.gradient().view(), right.gradient().view()],
            )
            .unwrap(),
        );

        Self {
            left,
            right,
            gradient,
            axis,
            overwrite: Cell::new(true),
        }
    }

    pub fn left_operand(&self) -> Rc<Lhs> {
        self.left.clone()
    }

    pub fn right_operand(&self) -> Rc<Rhs> {
        self.right.clone()
    }
}

impl<Lhs, Rhs> Gradient for ConcatenateBackward<Lhs, Rhs>
where
    Lhs: Gradient + Overwrite,
    Rhs: Gradient<Dim = Lhs::Dim> + Overwrite,
    Lhs::Dim: RemoveAxis,
{
    type Dim = Lhs::Dim;

    fn gradient(&self) -> Ref<Tensor<Self::Dim>> {
        self.gradient.borrow()
    }

    fn gradient_mut(&self) -> RefMut<Tensor<Self::Dim>> {
        self.gradient.borrow_mut()
    }
}

impl<Lhs, Rhs> Backward for ConcatenateBackward<Lhs, Rhs>
where
    Lhs: Gradient + Overwrite,
    Rhs: Gradient<Dim = Lhs::Dim> + Overwrite,
    Lhs::Dim: RemoveAxis,
{
    fn backward(&self) {
        let grad = self.gradient.borrow();
        let mut lhs_grad = self.left.gradient_mut();
        let mut rhs_grad = self.right.gradient_mut();
        let axis = self.axis;
        let (lhs_portion, rhs_portion) = grad
            .view()
            .split_at(Axis(axis), lhs_grad.len_of(Axis(axis)));

        let zip_lhs = Zip::from(&mut *lhs_grad).and(&lhs_portion);
        if self.left.can_overwrite() {
            zip_lhs.par_for_each(|lhs_grad_el, lhs_portion_el| *lhs_grad_el = *lhs_portion_el);
            self.left.set_overwrite(false);
        } else {
            zip_lhs.par_for_each(|lhs_grad_el, lhs_portion_el| *lhs_grad_el += *lhs_portion_el);
        }

        let zip_rhs = Zip::from(&mut *rhs_grad).and(&rhs_portion);
        if self.right.can_overwrite() {
            zip_rhs.par_for_each(|rhs_grad_el, rhs_portion_el| *rhs_grad_el = *rhs_portion_el);
            self.right.set_overwrite(false);
        } else {
            zip_rhs.par_for_each(|rhs_grad_el, rhs_portion_el| *rhs_grad_el += *rhs_portion_el);
        }
    }
}

impl<Lhs, Rhs> Overwrite for ConcatenateBackward<Lhs, Rhs>
where
    Lhs: Gradient + Overwrite,
    Rhs: Gradient<Dim = Lhs::Dim> + Overwrite,
    Lhs::Dim: RemoveAxis,
{
    fn can_overwrite(&self) -> bool {
        self.overwrite.get()
    }

    fn set_overwrite(&self, state: bool) {
        self.overwrite.set(state);
    }
}

// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~ ConcatenateBackwardLeft ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
pub struct ConcatenateBackwardLeft<T>
where
    T: Gradient + Overwrite,
    T::Dim: RemoveAxis,
{
    left: Rc<T>,
    axis: usize,
    gradient: RefCell<Tensor<T::Dim>>,
    overwrite: Cell<bool>,
}

impl<T> ConcatenateBackwardLeft<T>
where
    T: Gradient + Overwrite,
    T::Dim: RemoveAxis,
{
    pub fn new<U>(left: Rc<T>, right: Rc<U>, axis: usize) -> Self
    where
        U: Data<Dim = T::Dim>,
    {
        let gradient = RefCell::new(
            concatenate(Axis(axis), &[left.gradient().view(), right.data().view()]).unwrap(),
        );

        Self {
            left,
            axis,
            gradient,
            overwrite: Cell::new(true),
        }
    }

    pub fn operand(&self) -> Rc<T> {
        self.left.clone()
    }
}

impl<T> Gradient for ConcatenateBackwardLeft<T>
where
    T: Gradient + Overwrite,
    T::Dim: RemoveAxis,
{
    type Dim = T::Dim;

    fn gradient(&self) -> Ref<Tensor<Self::Dim>> {
        self.gradient.borrow()
    }

    fn gradient_mut(&self) -> RefMut<Tensor<Self::Dim>> {
        self.gradient.borrow_mut()
    }
}

impl<T> Backward for ConcatenateBackwardLeft<T>
where
    T: Gradient + Overwrite,
    T::Dim: RemoveAxis,
{
    fn backward(&self) {
        let grad = self.gradient.borrow();
        let mut lhs_grad = self.left.gradient_mut();
        let axis = self.axis;
        let (lhs_portion, _) = grad
            .view()
            .split_at(Axis(axis), lhs_grad.len_of(Axis(axis)));

        let zip_lhs = Zip::from(&mut *lhs_grad).and(&lhs_portion);
        if self.left.can_overwrite() {
            zip_lhs.par_for_each(|lhs_grad_el, lhs_portion_el| *lhs_grad_el = *lhs_portion_el);
            self.left.set_overwrite(false);
        } else {
            zip_lhs.par_for_each(|lhs_grad_el, lhs_portion_el| *lhs_grad_el += *lhs_portion_el);
        }
    }
}

impl<T> Overwrite for ConcatenateBackwardLeft<T>
where
    T: Gradient + Overwrite,
    T::Dim: RemoveAxis,
{
    fn can_overwrite(&self) -> bool {
        self.overwrite.get()
    }

    fn set_overwrite(&self, state: bool) {
        self.overwrite.set(state);
    }
}

// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~ ConcatenateBackwardRight ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

pub struct ConcatenateBackwardRight<T>
where
    T: Gradient + Overwrite,
    T::Dim: RemoveAxis,
{
    offset: usize,
    right: Rc<T>,
    axis: usize,
    gradient: RefCell<Tensor<T::Dim>>,
    overwrite: Cell<bool>,
}

impl<T> ConcatenateBackwardRight<T>
where
    T: Gradient + Overwrite,
    T::Dim: RemoveAxis,
{
    pub fn new<U>(left: Rc<U>, right: Rc<T>, axis: usize) -> Self
    where
        U: Data<Dim = T::Dim>,
    {
        let gradient = RefCell::new(
            concatenate(Axis(axis), &[left.data().view(), right.gradient().view()]).unwrap(),
        );

        Self {
            right,
            gradient,
            offset: left.data().len_of(Axis(axis)),
            axis,
            overwrite: Cell::new(true),
        }
    }
    pub fn operand(&self) -> Rc<T> {
        self.right.clone()
    }
}

impl<T> Gradient for ConcatenateBackwardRight<T>
where
    T: Gradient + Overwrite,
    T::Dim: RemoveAxis,
{
    type Dim = T::Dim;

    fn gradient(&self) -> Ref<Tensor<Self::Dim>> {
        self.gradient.borrow()
    }

    fn gradient_mut(&self) -> RefMut<Tensor<Self::Dim>> {
        self.gradient.borrow_mut()
    }
}

impl<T> Backward for ConcatenateBackwardRight<T>
where
    T: Gradient + Overwrite,
    T::Dim: RemoveAxis,
{
    fn backward(&self) {
        let grad = self.gradient.borrow();
        let mut rhs_grad = self.right.gradient_mut();
        let axis = self.axis;
        let (_, rhs_portion) = grad.view().split_at(Axis(axis), self.offset);

        let zip_rhs = Zip::from(&mut *rhs_grad).and(&rhs_portion);
        if self.right.can_overwrite() {
            zip_rhs.par_for_each(|rhs_grad_el, rhs_portion_el| *rhs_grad_el = *rhs_portion_el);
            self.right.set_overwrite(false);
        } else {
            zip_rhs.par_for_each(|rhs_grad_el, rhs_portion_el| *rhs_grad_el += *rhs_portion_el);
        }
    }
}

impl<T> Overwrite for ConcatenateBackwardRight<T>
where
    T: Gradient + Overwrite,
    T::Dim: RemoveAxis,
{
    fn can_overwrite(&self) -> bool {
        self.overwrite.get()
    }

    fn set_overwrite(&self, state: bool) {
        self.overwrite.set(state);
    }
}

// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~ StackBackward ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

pub struct StackBackward<Lhs, Rhs>
where
    Lhs: Gradient + Overwrite,
    Rhs: Gradient<Dim = Lhs::Dim> + Overwrite,
    Lhs::Dim: RemoveAxis,
{
    left: Rc<Lhs>,
    right: Rc<Rhs>,
    axis: usize,
    gradient: RefCell<Tensor<<Lhs::Dim as Dimension>::Larger>>,
    overwrite: Cell<bool>,
}

impl<Lhs, Rhs> StackBackward<Lhs, Rhs>
where
    Lhs: Gradient + Overwrite,
    Rhs: Gradient<Dim = Lhs::Dim> + Overwrite,
    Lhs::Dim: RemoveAxis,
{
    pub fn new(left: Rc<Lhs>, right: Rc<Rhs>, axis: usize) -> Self {
        let gradient = RefCell::new(
            stack(
                Axis(axis),
                &[left.gradient().view(), right.gradient().view()],
            )
            .unwrap(),
        );

        Self {
            left,
            right,
            gradient,
            axis,
            overwrite: Cell::new(true),
        }
    }

    pub fn left_operand(&self) -> Rc<Lhs> {
        self.left.clone()
    }

    pub fn right_operand(&self) -> Rc<Rhs> {
        self.right.clone()
    }
}

impl<Lhs, Rhs> Gradient for StackBackward<Lhs, Rhs>
where
    Lhs: Gradient + Overwrite,
    Rhs: Gradient<Dim = Lhs::Dim> + Overwrite,
    Lhs::Dim: RemoveAxis,
{
    type Dim = <Lhs::Dim as Dimension>::Larger;

    fn gradient(&self) -> Ref<Tensor<Self::Dim>> {
        self.gradient.borrow()
    }

    fn gradient_mut(&self) -> RefMut<Tensor<Self::Dim>> {
        self.gradient.borrow_mut()
    }
}

impl<Lhs, Rhs> Backward for StackBackward<Lhs, Rhs>
where
    Lhs: Gradient + Overwrite,
    Rhs: Gradient<Dim = Lhs::Dim> + Overwrite,
    Lhs::Dim: RemoveAxis,
{
    fn backward(&self) {
        let grad = self.gradient.borrow();
        let mut lhs_grad = self.left.gradient_mut();
        let mut rhs_grad = self.right.gradient_mut();
        let axis = self.axis;
        let mut subview_iter = grad.axis_iter(Axis(axis));
        let (lhs_portion, rhs_portion) = {
            (
                subview_iter
                    .next()
                    .unwrap()
                    .into_dimensionality::<Lhs::Dim>()
                    .unwrap(),
                subview_iter
                    .next()
                    .unwrap()
                    .into_dimensionality::<Rhs::Dim>()
                    .unwrap(),
            )
        };

        let zip_lhs = Zip::from(&mut *lhs_grad).and(&lhs_portion);
        if self.left.can_overwrite() {
            zip_lhs.par_for_each(|lhs_grad_el, lhs_portion_el| *lhs_grad_el = *lhs_portion_el);
            self.left.set_overwrite(false);
        } else {
            zip_lhs.par_for_each(|lhs_grad_el, lhs_portion_el| *lhs_grad_el += *lhs_portion_el);
        }

        let zip_rhs = Zip::from(&mut *rhs_grad).and(&rhs_portion);
        if self.right.can_overwrite() {
            zip_rhs.par_for_each(|rhs_grad_el, rhs_portion_el| *rhs_grad_el = *rhs_portion_el);
            self.right.set_overwrite(false);
        } else {
            zip_rhs.par_for_each(|rhs_grad_el, rhs_portion_el| *rhs_grad_el += *rhs_portion_el);
        }
    }
}

impl<Lhs, Rhs> Overwrite for StackBackward<Lhs, Rhs>
where
    Lhs: Gradient + Overwrite,
    Rhs: Gradient<Dim = Lhs::Dim> + Overwrite,
    Lhs::Dim: RemoveAxis,
{
    fn can_overwrite(&self) -> bool {
        self.overwrite.get()
    }

    fn set_overwrite(&self, state: bool) {
        self.overwrite.set(state);
    }
}

// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~ StackBackwardLeft ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
pub struct StackBackwardLeft<T>
where
    T: Gradient + Overwrite,
    T::Dim: RemoveAxis,
{
    left: Rc<T>,
    axis: usize,
    gradient: RefCell<Tensor<<T::Dim as Dimension>::Larger>>,
    overwrite: Cell<bool>,
}

impl<T> StackBackwardLeft<T>
where
    T: Gradient + Overwrite,
    T::Dim: RemoveAxis,
{
    pub fn new<U>(left: Rc<T>, right: Rc<U>, axis: usize) -> Self
    where
        U: Data<Dim = T::Dim>,
    {
        let gradient = RefCell::new(
            stack(Axis(axis), &[left.gradient().view(), right.data().view()]).unwrap(),
        );

        Self {
            left,
            gradient,
            axis,
            overwrite: Cell::new(true),
        }
    }

    pub fn operand(&self) -> Rc<T> {
        self.left.clone()
    }
}

impl<T> Gradient for StackBackwardLeft<T>
where
    T: Gradient + Overwrite,
    T::Dim: RemoveAxis,
{
    type Dim = <T::Dim as Dimension>::Larger;

    fn gradient(&self) -> Ref<Tensor<Self::Dim>> {
        self.gradient.borrow()
    }

    fn gradient_mut(&self) -> RefMut<Tensor<Self::Dim>> {
        self.gradient.borrow_mut()
    }
}

impl<T> Backward for StackBackwardLeft<T>
where
    T: Gradient + Overwrite,
    T::Dim: RemoveAxis,
{
    fn backward(&self) {
        let grad = self.gradient.borrow();
        let mut lhs_grad = self.left.gradient_mut();
        let axis = self.axis;
        let mut subview_iter = grad.axis_iter(Axis(axis));
        let lhs_portion = subview_iter
            .next()
            .unwrap()
            .into_dimensionality::<T::Dim>()
            .unwrap();

        let zip_lhs = Zip::from(&mut *lhs_grad).and(&lhs_portion);
        if self.left.can_overwrite() {
            zip_lhs.par_for_each(|lhs_grad_el, lhs_portion_el| *lhs_grad_el = *lhs_portion_el);
            self.left.set_overwrite(false);
        } else {
            zip_lhs.par_for_each(|lhs_grad_el, lhs_portion_el| *lhs_grad_el += *lhs_portion_el);
        }
    }
}

impl<T> Overwrite for StackBackwardLeft<T>
where
    T: Gradient + Overwrite,
    T::Dim: RemoveAxis,
{
    fn can_overwrite(&self) -> bool {
        self.overwrite.get()
    }

    fn set_overwrite(&self, state: bool) {
        self.overwrite.set(state);
    }
}

// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~ StackBackwardRight ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

pub struct StackBackwardRight<T>
where
    T: Gradient + Overwrite,
    T::Dim: RemoveAxis,
{
    right: Rc<T>,
    axis: usize,
    gradient: RefCell<Tensor<<T::Dim as Dimension>::Larger>>,
    overwrite: Cell<bool>,
}

impl<T> StackBackwardRight<T>
where
    T: Gradient + Overwrite,
    T::Dim: RemoveAxis,
{
    pub fn new<U>(left: Rc<U>, right: Rc<T>, axis: usize) -> Self
    where
        U: Data<Dim = T::Dim>,
    {
        let gradient = RefCell::new(
            stack(Axis(axis), &[left.data().view(), right.gradient().view()]).unwrap(),
        );

        Self {
            right,
            gradient,
            axis,
            overwrite: Cell::new(true),
        }
    }

    pub fn operand(&self) -> Rc<T> {
        self.right.clone()
    }
}

impl<T> Gradient for StackBackwardRight<T>
where
    T: Gradient + Overwrite,
    T::Dim: RemoveAxis,
{
    type Dim = <T::Dim as Dimension>::Larger;

    fn gradient(&self) -> Ref<Tensor<Self::Dim>> {
        self.gradient.borrow()
    }

    fn gradient_mut(&self) -> RefMut<Tensor<Self::Dim>> {
        self.gradient.borrow_mut()
    }
}

impl<T> Backward for StackBackwardRight<T>
where
    T: Gradient + Overwrite,
    T::Dim: RemoveAxis,
{
    fn backward(&self) {
        let grad = self.gradient.borrow();
        let mut rhs_grad = self.right.gradient_mut();
        let axis = self.axis;
        let rhs_portion = grad
            .axis_iter(Axis(axis))
            .nth(1)
            .unwrap()
            .into_dimensionality::<T::Dim>()
            .unwrap();

        let zip_rhs = Zip::from(&mut *rhs_grad).and(&rhs_portion);
        if self.right.can_overwrite() {
            zip_rhs.par_for_each(|rhs_grad_el, rhs_portion_el| *rhs_grad_el = *rhs_portion_el);
            self.right.set_overwrite(false);
        } else {
            zip_rhs.par_for_each(|rhs_grad_el, rhs_portion_el| *rhs_grad_el += *rhs_portion_el);
        }
    }
}

impl<T> Overwrite for StackBackwardRight<T>
where
    T: Gradient + Overwrite,
    T::Dim: RemoveAxis,
{
    fn can_overwrite(&self) -> bool {
        self.overwrite.get()
    }

    fn set_overwrite(&self, state: bool) {
        self.overwrite.set(state);
    }
}

// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~ UnsqueezeBackward ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

pub struct UnsqueezeBackward<T: Gradient + Overwrite> {
    operand: Rc<T>,
    axis: usize,
    gradient: RefCell<Tensor<<T::Dim as Dimension>::Larger>>,
    overwrite: Cell<bool>,
}

impl<T: Gradient + Overwrite> UnsqueezeBackward<T> {
    pub fn new(operand: Rc<T>, axis: usize) -> Self {
        let shape = operand.gradient().raw_dim();
        let gradient = RefCell::new(Tensor::zeros(shape.insert_axis(Axis(axis))));

        Self {
            operand,
            axis,
            gradient,
            overwrite: Cell::new(true),
        }
    }
}

impl<T: Gradient + Overwrite> Gradient for UnsqueezeBackward<T> {
    type Dim = <T::Dim as Dimension>::Larger;

    fn gradient(&self) -> Ref<Tensor<Self::Dim>> {
        self.gradient.borrow()
    }

    fn gradient_mut(&self) -> RefMut<Tensor<Self::Dim>> {
        self.gradient.borrow_mut()
    }
}

impl<T: Gradient + Overwrite> Backward for UnsqueezeBackward<T> {
    fn backward(&self) {
        let mut operand_grad = self.operand.gradient_mut();
        let axis = self.axis;
        let grad = self.gradient.borrow();
        let unsqueezed_gradient = grad
            .axis_iter(Axis(axis))
            .next()
            .unwrap()
            .into_dimensionality::<T::Dim>()
            .unwrap();

        let zip = Zip::from(&mut *operand_grad).and(&unsqueezed_gradient);
        if self.operand.can_overwrite() {
            zip.par_for_each(|dest, src| *dest = *src);
            self.operand.set_overwrite(false);
        } else {
            zip.par_for_each(|dest, src| *dest += src);
        }
    }
}

impl<T: Gradient + Overwrite> Overwrite for UnsqueezeBackward<T> {
    fn can_overwrite(&self) -> bool {
        self.overwrite.get()
    }

    fn set_overwrite(&self, state: bool) {
        self.overwrite.set(state);
    }
}

// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~ ChunkBackward ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

pub struct ChunkBackward<T: Gradient + Overwrite> {
    operand: Rc<T>,
    chunk_no: usize,
    chunk_shape: T::Dim,
    gradient: RefCell<Tensor<T::Dim>>,
    overwrite: Cell<bool>,
}

impl<T: Gradient + Overwrite> ChunkBackward<T> {
    pub fn new(operand: Rc<T>, grad_chunk: Tensor<T::Dim>, chunk_no: usize) -> Self {
        Self {
            operand,
            chunk_no,
            chunk_shape: grad_chunk.raw_dim(),
            gradient: RefCell::new(grad_chunk),
            overwrite: Cell::new(true),
        }
    }
}

impl<T: Gradient + Overwrite> Gradient for ChunkBackward<T> {
    type Dim = T::Dim;

    fn gradient(&self) -> Ref<Tensor<Self::Dim>> {
        self.gradient.borrow()
    }

    fn gradient_mut(&self) -> RefMut<Tensor<Self::Dim>> {
        self.gradient.borrow_mut()
    }
}

impl<T: Gradient + Overwrite> Backward for ChunkBackward<T> {
    fn backward(&self) {
        let mut operand_grad = self.operand.gradient_mut();
        let grad = self.gradient.borrow();
        let (chunk_no, chunk_shape) = (self.chunk_no, &self.chunk_shape);
        let mut op_gradient_chunk = operand_grad
            .exact_chunks_mut(chunk_shape.clone())
            .into_iter()
            .skip(chunk_no)
            .take(1)
            .next()
            .unwrap();

        let zip = Zip::from(&mut op_gradient_chunk).and(&*grad);
        if self.operand.can_overwrite() {
            zip.par_for_each(|dest, src| *dest = *src);
            self.operand.set_overwrite(false);
        } else {
            zip.par_for_each(|dest, src| *dest += src);
        }
    }
}

impl<T: Gradient + Overwrite> Overwrite for ChunkBackward<T> {
    fn can_overwrite(&self) -> bool {
        self.overwrite.get()
    }

    fn set_overwrite(&self, state: bool) {
        self.overwrite.set(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ndarray::StrideShape;

    const F16_EPSILON: f32 = 9.77e-04;

    fn assert_almost_equals<D: Dimension>(our: &Tensor<D>, their: &Tensor<D>) {
        assert!(
            Zip::from(our).and(their).all(|l, r| {
                (*l == 0. && *r == 0.)
                    || (!l.is_finite() && !r.is_finite())
                    || ((1. - r / l).abs() <= F16_EPSILON)
            }),
            "\nLeft:\n{}\nRight:\n{}",
            our,
            their
        );
    }

    fn new_input<D, Sh>(shape: Sh, elems: Vec<f32>) -> Rc<Input<D>>
    where
        D: Dimension + 'static,
        Sh: Into<StrideShape<D>>,
    {
        Input::new(new_tensor(shape, elems)).last
    }

    fn new_backward_input<D, Sh>(shape: Sh, elems: Vec<f32>) -> Rc<InputBackward<D>>
    where
        D: Dimension + 'static,
        Sh: Into<StrideShape<D>>,
    {
        Rc::new(Input::new(new_tensor(shape, elems)).last.differentiable())
    }

    fn new_tensor<D, Sh>(shape: Sh, elems: Vec<f32>) -> Tensor<D>
    where
        D: Dimension + 'static,
        Sh: Into<StrideShape<D>>,
    {
        Tensor::from_shape_vec(shape, elems).unwrap()
    }

    mod backward_negation {
        use super::*;

        #[test]
        fn creation() {
            let input = new_backward_input((3, 3), vec![0.; 9]);
            let node = NegationBackward::new(input);

            assert_eq!(*node.gradient(), Tensor::from_elem((3, 3), 0.));
            assert_eq!(node.can_overwrite(), true);
        }

        #[test]
        fn computation_state_transition() {
            let input = new_backward_input((3, 3), vec![0.; 9]);
            let node = NegationBackward::new(input.clone());

            node.backward();
            assert_eq!(input.can_overwrite(), false);

            node.backward();
            assert_eq!(input.can_overwrite(), false);

            node.set_overwrite(true);
            assert_eq!(input.can_overwrite(), false);

            node.set_overwrite(true);
            assert_eq!(input.can_overwrite(), false);

            input.set_overwrite(true);
            assert_eq!(input.can_overwrite(), true);

            input.set_overwrite(true);
            assert_eq!(input.can_overwrite(), true);
        }

        #[test]
        fn backward() {
            let input = new_backward_input((3, 3), vec![0.; 9]);
            let node = NegationBackward::new(input.clone());

            // -------------------------------------- Seed Gradient --------------------------------------
            *node.gradient_mut() = new_tensor((3, 3), vec![1.; 9]);
            assert_almost_equals(&*node.gradient(), &new_tensor((3, 3), vec![1.; 9]));

            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(&*input.gradient(), &new_tensor((3, 3), vec![-1.; 9]));

            // ----------------------------------- No Second Evaluation -----------------------------------
            *node.gradient_mut() = new_tensor((3, 3), vec![-1.; 9]);
            assert_almost_equals(&*node.gradient(), &new_tensor((3, 3), vec![-1.; 9]));
            assert_almost_equals(&*input.gradient(), &new_tensor((3, 3), vec![-1.; 9]));

            // ------------------------------------- Second Evaluation -------------------------------------
            input.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(&*input.gradient(), &new_tensor((3, 3), vec![1.; 9]));
        }
    }
    mod backward_addition {
        use super::*;

        #[test]
        fn backward() {
            let (lhs, rhs) = {
                (
                    new_backward_input((3, 3), vec![0.; 9]),
                    new_backward_input((3, 3), vec![0.; 9]),
                )
            };
            let node = AdditionBackward::new(lhs.clone(), rhs.clone());
            *node.gradient_mut() = new_tensor((3, 3), vec![1.; 9]);
            // -------------------------------------- Seed Gradient --------------------------------------
            assert_almost_equals(&*node.gradient(), &new_tensor((3, 3), vec![1.; 9]));
            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(&*lhs.gradient(), &new_tensor((3, 3), vec![1.; 9]));
            assert_almost_equals(&*rhs.gradient(), &new_tensor((3, 3), vec![1.; 9]));
            // ------------------------------------- Second Evaluation -------------------------------------
            lhs.set_overwrite(true);
            rhs.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(&*lhs.gradient(), &new_tensor((3, 3), vec![1.; 9]));
            assert_almost_equals(&*rhs.gradient(), &new_tensor((3, 3), vec![1.; 9]));
        }

        #[test]
        fn backward_broadcast_left() {
            let (lhs, rhs) = {
                (
                    new_backward_input(3, vec![0.; 3]),
                    new_backward_input((3, 3), vec![0.; 9]),
                )
            };
            let node = AdditionBackward::new(lhs.clone(), rhs.clone());
            *node.gradient_mut() = new_tensor((3, 3), vec![1.; 9]);
            // -------------------------------------- Seed Gradient --------------------------------------
            assert_almost_equals(&*node.gradient(), &new_tensor((3, 3), vec![1.; 9]));
            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(&*lhs.gradient(), &new_tensor(3, vec![3.; 3]));
            assert_almost_equals(&*rhs.gradient(), &new_tensor((3, 3), vec![1.; 9]));
            // ------------------------------------- Second Evaluation -------------------------------------
            lhs.set_overwrite(true);
            rhs.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(&*lhs.gradient(), &new_tensor(3, vec![3.; 3]));
            assert_almost_equals(&*rhs.gradient(), &new_tensor((3, 3), vec![1.; 9]));
        }

        #[test]
        fn backward_broadcast_right() {
            let (lhs, rhs) = {
                (
                    new_backward_input((3, 3), vec![0.; 9]),
                    new_backward_input(3, vec![0.; 3]),
                )
            };
            let node = AdditionBackward::new(lhs.clone(), rhs.clone());
            *node.gradient_mut() = new_tensor((3, 3), vec![1.; 9]);
            // -------------------------------------- Seed Gradient --------------------------------------
            assert_almost_equals(&*node.gradient(), &new_tensor((3, 3), vec![1.; 9]));
            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(&*lhs.gradient(), &new_tensor((3, 3), vec![1.; 9]));
            assert_almost_equals(&*rhs.gradient(), &new_tensor(3, vec![3.; 3]));
            // ------------------------------------- Second Evaluation -------------------------------------
            lhs.set_overwrite(true);
            rhs.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(&*lhs.gradient(), &new_tensor((3, 3), vec![1.; 9]));
            assert_almost_equals(&*rhs.gradient(), &new_tensor(3, vec![3.; 3]));
        }

        #[test]
        fn backward_unary() {
            let (input_diff, input_not_diff) = {
                (
                    new_backward_input((3, 3), vec![0.; 9]),
                    new_input((3, 3), vec![0.; 9]),
                )
            };
            let node = AdditionBackwardUnary::new(input_diff.clone(), input_not_diff);
            *node.gradient_mut() = new_tensor((3, 3), vec![1.; 9]);
            // -------------------------------------- Seed Gradient --------------------------------------
            assert_almost_equals(&*node.gradient(), &new_tensor((3, 3), vec![1.; 9]));
            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(&*input_diff.gradient(), &new_tensor((3, 3), vec![1.; 9]));
            // ------------------------------------- Second Evaluation -------------------------------------
            input_diff.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(&*input_diff.gradient(), &new_tensor((3, 3), vec![1.; 9]));
        }

        #[test]
        fn backward_unary_broadcast() {
            let (input_diff, input_not_diff) = {
                (
                    new_backward_input(3, vec![0.; 3]),
                    new_input((3, 3), vec![0.; 9]),
                )
            };
            let node = AdditionBackwardUnary::new(input_diff.clone(), input_not_diff);
            *node.gradient_mut() = new_tensor((3, 3), vec![1.; 9]);
            // -------------------------------------- Seed Gradient --------------------------------------
            assert_almost_equals(&*node.gradient(), &new_tensor((3, 3), vec![1.; 9]));
            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(&*input_diff.gradient(), &new_tensor(3, vec![3.; 3]));
            // ------------------------------------- Second Evaluation -------------------------------------
            input_diff.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(&*input_diff.gradient(), &new_tensor(3, vec![3.; 3]));
        }
    }

    mod backward_subtraction {
        use super::*;
        #[test]
        fn backward() {
            let (lhs, rhs) = {
                (
                    new_backward_input((3, 3), vec![0.; 9]),
                    new_backward_input((3, 3), vec![0.; 9]),
                )
            };
            let node = SubtractionBackward::new(lhs.clone(), rhs.clone());
            *node.gradient_mut() = new_tensor((3, 3), vec![1.; 9]);
            // -------------------------------------- Seed Gradient --------------------------------------
            assert_almost_equals(&*node.gradient(), &new_tensor((3, 3), vec![1.; 9]));
            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(&*lhs.gradient(), &new_tensor((3, 3), vec![1.; 9]));
            assert_almost_equals(&*rhs.gradient(), &new_tensor((3, 3), vec![-1.; 9]));
            // ------------------------------------- Second Evaluation -------------------------------------
            lhs.set_overwrite(true);
            rhs.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(&*lhs.gradient(), &new_tensor((3, 3), vec![1.; 9]));
            assert_almost_equals(&*rhs.gradient(), &new_tensor((3, 3), vec![-1.; 9]));
        }

        #[test]
        fn backward_broadcast_left() {
            let (lhs, rhs) = {
                (
                    new_backward_input(3, vec![0.; 3]),
                    new_backward_input((3, 3), vec![0.; 9]),
                )
            };
            let node = SubtractionBackward::new(lhs.clone(), rhs.clone());
            *node.gradient_mut() = new_tensor((3, 3), vec![1.; 9]);
            // -------------------------------------- Seed Gradient --------------------------------------
            assert_almost_equals(&*node.gradient(), &new_tensor((3, 3), vec![1.; 9]));
            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(&*lhs.gradient(), &new_tensor(3, vec![3.; 3]));
            assert_almost_equals(&*rhs.gradient(), &new_tensor((3, 3), vec![-1.; 9]));
            // ------------------------------------- Second Evaluation -------------------------------------
            lhs.set_overwrite(true);
            rhs.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(&*lhs.gradient(), &new_tensor(3, vec![3.; 3]));
            assert_almost_equals(&*rhs.gradient(), &new_tensor((3, 3), vec![-1.; 9]));
        }

        #[test]
        fn backward_broadcast_right() {
            let (lhs, rhs) = {
                (
                    new_backward_input((3, 3), vec![0.; 9]),
                    new_backward_input(3, vec![0.; 3]),
                )
            };
            let node = SubtractionBackward::new(lhs.clone(), rhs.clone());
            *node.gradient_mut() = new_tensor((3, 3), vec![1.; 9]);
            // -------------------------------------- Seed Gradient --------------------------------------
            assert_almost_equals(&*node.gradient(), &new_tensor((3, 3), vec![1.; 9]));
            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(&*lhs.gradient(), &new_tensor((3, 3), vec![1.; 9]));
            assert_almost_equals(&*rhs.gradient(), &new_tensor(3, vec![-3.; 3]));
            // ------------------------------------- Second Evaluation -------------------------------------
            lhs.set_overwrite(true);
            rhs.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(&*lhs.gradient(), &new_tensor((3, 3), vec![1.; 9]));
            assert_almost_equals(&*rhs.gradient(), &new_tensor(3, vec![-3.; 3]));
        }

        #[test]
        fn backward_left() {
            let (input_diff, input_not_diff) = {
                (
                    new_backward_input((3, 3), vec![0.; 9]),
                    new_input((3, 3), vec![0.; 9]),
                )
            };
            let node = SubtractionBackwardLeft::new(input_diff.clone(), input_not_diff);
            *node.gradient_mut() = new_tensor((3, 3), vec![1.; 9]);
            // -------------------------------------- Seed Gradient --------------------------------------
            assert_almost_equals(&*node.gradient(), &new_tensor((3, 3), vec![1.; 9]));
            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(&*input_diff.gradient(), &new_tensor((3, 3), vec![1.; 9]));
            // ------------------------------------- Second Evaluation -------------------------------------
            input_diff.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(&*input_diff.gradient(), &new_tensor((3, 3), vec![1.; 9]));
        }

        #[test]
        fn backward_left_broadcast() {
            let (input_diff, input_not_diff) = {
                (
                    new_backward_input(3, vec![0.; 3]),
                    new_input((3, 3), vec![0.; 9]),
                )
            };
            let node = SubtractionBackwardLeft::new(input_diff.clone(), input_not_diff);
            *node.gradient_mut() = new_tensor((3, 3), vec![1.; 9]);
            // -------------------------------------- Seed Gradient --------------------------------------
            assert_almost_equals(&*node.gradient(), &new_tensor((3, 3), vec![1.; 9]));
            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(&*input_diff.gradient(), &new_tensor(3, vec![3.; 3]));
            // ------------------------------------- Second Evaluation -------------------------------------
            input_diff.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(&*input_diff.gradient(), &new_tensor(3, vec![3.; 3]));
        }

        #[test]
        fn backward_right() {
            let (input_diff, input_not_diff) = {
                (
                    new_backward_input((3, 3), vec![0.; 9]),
                    new_input((3, 3), vec![0.; 9]),
                )
            };
            let node = SubtractionBackwardRight::new(input_diff.clone(), input_not_diff);
            *node.gradient_mut() = new_tensor((3, 3), vec![1.; 9]);
            // -------------------------------------- Seed Gradient --------------------------------------
            assert_almost_equals(&*node.gradient(), &new_tensor((3, 3), vec![1.; 9]));
            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(&*input_diff.gradient(), &new_tensor((3, 3), vec![-1.; 9]));
            // ------------------------------------- Second Evaluation -------------------------------------
            input_diff.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(&*input_diff.gradient(), &new_tensor((3, 3), vec![-1.; 9]));
        }

        #[test]
        fn backward_right_broadcast() {
            let (input_diff, input_not_diff) = {
                (
                    new_backward_input(3, vec![0.; 3]),
                    new_input((3, 3), vec![0.; 9]),
                )
            };
            let node = SubtractionBackwardRight::new(input_diff.clone(), input_not_diff);
            *node.gradient_mut() = new_tensor((3, 3), vec![1.; 9]);
            // -------------------------------------- Seed Gradient --------------------------------------
            assert_almost_equals(&*node.gradient(), &new_tensor((3, 3), vec![1.; 9]));
            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(&*input_diff.gradient(), &new_tensor(3, vec![-3.; 3]));
            // ------------------------------------- Second Evaluation -------------------------------------
            input_diff.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(&*input_diff.gradient(), &new_tensor(3, vec![-3.; 3]));
        }
    }

    mod backward_multiplication {
        use super::*;
        #[test]
        fn backward() {
            let (lhs_f, lhs_b, rhs_f, rhs_b) = {
                (
                    new_input((3, 3), vec![3.; 9]),
                    new_backward_input((3, 3), vec![0.; 9]),
                    new_input((3, 3), vec![5.; 9]),
                    new_backward_input((3, 3), vec![0.; 9]),
                )
            };
            let node = MultiplicationBackward::new(lhs_f, lhs_b.clone(), rhs_f, rhs_b.clone());
            *node.gradient_mut() = new_tensor((3, 3), vec![1.; 9]);
            // -------------------------------------- Seed Gradient --------------------------------------
            assert_almost_equals(&*node.gradient(), &new_tensor((3, 3), vec![1.; 9]));
            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(&*lhs_b.gradient(), &new_tensor((3, 3), vec![5.; 9]));
            assert_almost_equals(&*rhs_b.gradient(), &new_tensor((3, 3), vec![3.; 9]));
            // ------------------------------------- Second Evaluation -------------------------------------
            lhs_b.set_overwrite(true);
            rhs_b.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(&*lhs_b.gradient(), &new_tensor((3, 3), vec![5.; 9]));
            assert_almost_equals(&*rhs_b.gradient(), &new_tensor((3, 3), vec![3.; 9]));
        }

        #[test]
        fn backward_broadcast_left() {
            let (lhs_f, lhs_b, rhs_f, rhs_b) = {
                (
                    new_input(3, vec![3.; 3]),
                    new_backward_input(3, vec![0.; 3]),
                    new_input((3, 3), vec![5.; 9]),
                    new_backward_input((3, 3), vec![0.; 9]),
                )
            };
            let node = MultiplicationBackward::new(lhs_f, lhs_b.clone(), rhs_f, rhs_b.clone());
            *node.gradient_mut() = new_tensor((3, 3), vec![1.; 9]);
            // -------------------------------------- Seed Gradient --------------------------------------
            assert_almost_equals(&*node.gradient(), &new_tensor((3, 3), vec![1.; 9]));
            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(&*lhs_b.gradient(), &new_tensor(3, vec![15.; 3]));
            assert_almost_equals(&*rhs_b.gradient(), &new_tensor((3, 3), vec![3.; 9]));
            // ------------------------------------- Second Evaluation -------------------------------------
            lhs_b.set_overwrite(true);
            rhs_b.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(&*lhs_b.gradient(), &new_tensor(3, vec![15.; 3]));
            assert_almost_equals(&*rhs_b.gradient(), &new_tensor((3, 3), vec![3.; 9]));
        }

        #[test]
        fn backward_broadcast_right() {
            let (lhs_f, lhs_b, rhs_f, rhs_b) = {
                (
                    new_input((3, 3), vec![3.; 9]),
                    new_backward_input((3, 3), vec![0.; 9]),
                    new_input(3, vec![5.; 3]),
                    new_backward_input(3, vec![0.; 3]),
                )
            };
            let node = MultiplicationBackward::new(lhs_f, lhs_b.clone(), rhs_f, rhs_b.clone());
            *node.gradient_mut() = new_tensor((3, 3), vec![1.; 9]);
            // -------------------------------------- Seed Gradient --------------------------------------
            assert_almost_equals(&*node.gradient(), &new_tensor((3, 3), vec![1.; 9]));
            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(&*lhs_b.gradient(), &new_tensor((3, 3), vec![5.; 9]));
            assert_almost_equals(&*rhs_b.gradient(), &new_tensor(3, vec![9.; 3]));
            // ------------------------------------- Second Evaluation -------------------------------------
            lhs_b.set_overwrite(true);
            rhs_b.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(&*lhs_b.gradient(), &new_tensor((3, 3), vec![5.; 9]));
            assert_almost_equals(&*rhs_b.gradient(), &new_tensor(3, vec![9.; 3]));
        }

        #[test]
        fn backward_unary() {
            let (input_diff, input_not_diff) = {
                (
                    new_backward_input((3, 3), vec![0.; 9]),
                    new_input((3, 3), vec![5.; 9]),
                )
            };
            let node = MultiplicationBackwardUnary::new(input_diff.clone(), input_not_diff);
            *node.gradient_mut() = new_tensor((3, 3), vec![1.; 9]);
            // -------------------------------------- Seed Gradient --------------------------------------
            assert_almost_equals(&*node.gradient(), &new_tensor((3, 3), vec![1.; 9]));
            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(&*input_diff.gradient(), &new_tensor((3, 3), vec![5.; 9]));
            // ------------------------------------- Second Evaluation -------------------------------------
            input_diff.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(&*input_diff.gradient(), &new_tensor((3, 3), vec![5.; 9]));
        }

        #[test]
        fn backward_unary_broadcast() {
            let (input_diff, input_not_diff) = {
                (
                    new_backward_input(3, vec![0.; 3]),
                    new_input((3, 3), vec![5.; 9]),
                )
            };
            let node = MultiplicationBackwardUnary::new(input_diff.clone(), input_not_diff);
            *node.gradient_mut() = new_tensor((3, 3), vec![1.; 9]);
            // -------------------------------------- Seed Gradient --------------------------------------
            assert_almost_equals(&*node.gradient(), &new_tensor((3, 3), vec![1.; 9]));
            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(&*input_diff.gradient(), &new_tensor(3, vec![15.; 3]));
            // ------------------------------------- Second Evaluation -------------------------------------
            input_diff.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(&*input_diff.gradient(), &new_tensor(3, vec![15.; 3]));
        }
    }

    mod backward_division {
        use super::*;
        #[test]
        fn backward() {
            let (lhs_f, lhs_b, rhs_f, rhs_b) = {
                (
                    new_input((3, 3), vec![3.; 9]),
                    new_backward_input((3, 3), vec![0.; 9]),
                    new_input((3, 3), vec![5.; 9]),
                    new_backward_input((3, 3), vec![0.; 9]),
                )
            };
            let node = DivisionBackward::new(lhs_f, lhs_b.clone(), rhs_f, rhs_b.clone());
            *node.gradient_mut() = new_tensor((3, 3), vec![1.; 9]);
            // -------------------------------------- Seed Gradient --------------------------------------
            assert_almost_equals(&*node.gradient(), &new_tensor((3, 3), vec![1.; 9]));
            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(&*lhs_b.gradient(), &new_tensor((3, 3), vec![0.2; 9]));
            assert_almost_equals(&*rhs_b.gradient(), &new_tensor((3, 3), vec![-0.12; 9]));
            // ------------------------------------- Second Evaluation -------------------------------------
            lhs_b.set_overwrite(true);
            rhs_b.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(&*lhs_b.gradient(), &new_tensor((3, 3), vec![0.2; 9]));
            assert_almost_equals(&*rhs_b.gradient(), &new_tensor((3, 3), vec![-0.12; 9]));
        }

        #[test]
        fn backward_broadcast_left() {
            let (lhs_f, lhs_b, rhs_f, rhs_b) = {
                (
                    new_input(3, vec![3.; 3]),
                    new_backward_input(3, vec![0.; 3]),
                    new_input((3, 3), vec![5.; 9]),
                    new_backward_input((3, 3), vec![0.; 9]),
                )
            };
            let node = DivisionBackward::new(lhs_f, lhs_b.clone(), rhs_f, rhs_b.clone());
            *node.gradient_mut() = new_tensor((3, 3), vec![1.; 9]);
            // -------------------------------------- Seed Gradient --------------------------------------
            assert_almost_equals(&*node.gradient(), &new_tensor((3, 3), vec![1.; 9]));
            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(&*lhs_b.gradient(), &new_tensor(3, vec![0.6; 3]));
            assert_almost_equals(&*rhs_b.gradient(), &new_tensor((3, 3), vec![-0.12; 9]));
            // ------------------------------------- Second Evaluation -------------------------------------
            lhs_b.set_overwrite(true);
            rhs_b.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(&*lhs_b.gradient(), &new_tensor(3, vec![0.6; 3]));
            assert_almost_equals(&*rhs_b.gradient(), &new_tensor((3, 3), vec![-0.12; 9]));
        }

        #[test]
        fn backward_broadcast_right() {
            let (lhs_f, lhs_b, rhs_f, rhs_b) = {
                (
                    new_input((3, 3), vec![3.; 9]),
                    new_backward_input((3, 3), vec![0.; 9]),
                    new_input(3, vec![5.; 3]),
                    new_backward_input(3, vec![0.; 3]),
                )
            };
            let node = DivisionBackward::new(lhs_f, lhs_b.clone(), rhs_f, rhs_b.clone());
            *node.gradient_mut() = new_tensor((3, 3), vec![1.; 9]);
            // -------------------------------------- Seed Gradient --------------------------------------
            assert_almost_equals(&*node.gradient(), &new_tensor((3, 3), vec![1.; 9]));
            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(&*lhs_b.gradient(), &new_tensor((3, 3), vec![0.2; 9]));
            assert_almost_equals(&*rhs_b.gradient(), &new_tensor(3, vec![-0.36; 3]));
            // ------------------------------------- Second Evaluation -------------------------------------
            lhs_b.set_overwrite(true);
            rhs_b.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(&*lhs_b.gradient(), &new_tensor((3, 3), vec![0.2; 9]));
            assert_almost_equals(&*rhs_b.gradient(), &new_tensor(3, vec![-0.36; 3]));
        }

        #[test]
        fn backward_left() {
            let (input_diff, input_not_diff) = {
                (
                    new_backward_input((3, 3), vec![0.; 9]),
                    new_input((3, 3), vec![5.; 9]),
                )
            };
            let node = DivisionBackwardLeft::new(input_diff.clone(), input_not_diff);
            *node.gradient_mut() = new_tensor((3, 3), vec![1.; 9]);
            // -------------------------------------- Seed Gradient --------------------------------------
            assert_almost_equals(&*node.gradient(), &new_tensor((3, 3), vec![1.; 9]));
            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(&*input_diff.gradient(), &new_tensor((3, 3), vec![0.2; 9]));
            // ------------------------------------- Second Evaluation -------------------------------------
            input_diff.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(&*input_diff.gradient(), &new_tensor((3, 3), vec![0.2; 9]));
        }

        #[test]
        fn backward_left_broadcast() {
            let (input_diff, input_not_diff) = {
                (
                    new_backward_input(3, vec![0.; 3]),
                    new_input((3, 3), vec![5.; 9]),
                )
            };
            let node = DivisionBackwardLeft::new(input_diff.clone(), input_not_diff);
            *node.gradient_mut() = new_tensor((3, 3), vec![1.; 9]);
            // -------------------------------------- Seed Gradient --------------------------------------
            assert_almost_equals(&*node.gradient(), &new_tensor((3, 3), vec![1.; 9]));
            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(&*input_diff.gradient(), &new_tensor(3, vec![0.6; 3]));
            // ------------------------------------- Second Evaluation -------------------------------------
            input_diff.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(&*input_diff.gradient(), &new_tensor(3, vec![0.6; 3]));
        }

        #[test]
        fn backward_right() {
            let (input, input_diff, input_not_diff) = {
                (
                    new_input((3, 3), vec![5.; 9]),
                    new_backward_input((3, 3), vec![0.; 9]),
                    new_input((3, 3), vec![3.; 9]),
                )
            };
            let node = DivisionBackwardRight::new(input_not_diff, input, input_diff.clone());
            *node.gradient_mut() = new_tensor((3, 3), vec![1.; 9]);
            // -------------------------------------- Seed Gradient --------------------------------------
            assert_almost_equals(&*node.gradient(), &new_tensor((3, 3), vec![1.; 9]));
            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(&*input_diff.gradient(), &new_tensor((3, 3), vec![-0.12; 9]));
            // ------------------------------------- Second Evaluation -------------------------------------
            input_diff.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(&*input_diff.gradient(), &new_tensor((3, 3), vec![-0.12; 9]));
        }

        #[test]
        fn backward_right_broadcast() {
            let (input, input_diff, input_not_diff) = {
                (
                    new_input((3, 3), vec![5.; 9]),
                    new_backward_input(3, vec![0.; 3]),
                    new_input((3, 3), vec![3.; 9]),
                )
            };
            let node = DivisionBackwardRight::new(input_not_diff, input, input_diff.clone());
            *node.gradient_mut() = new_tensor((3, 3), vec![1.; 9]);
            // -------------------------------------- Seed Gradient --------------------------------------
            assert_almost_equals(&*node.gradient(), &new_tensor((3, 3), vec![1.; 9]));
            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(&*input_diff.gradient(), &new_tensor(3, vec![-0.36; 3]));
            // ------------------------------------- Second Evaluation -------------------------------------
            input_diff.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(&*input_diff.gradient(), &new_tensor(3, vec![-0.36; 3]));
        }
    }

    mod backward_matrix_matrix_mul {
        use super::*;
        #[test]
        fn backward() {
            let (lhs_f, lhs_b, rhs_f, rhs_b) = {
                (
                    new_input((3, 3), vec![1., 2., 3., 4., 5., 6., 7., 8., 9.]),
                    new_backward_input((3, 3), vec![0.; 9]),
                    new_input((3, 3), vec![10., 11., 12., 13., 14., 15., 16., 17., 18.]),
                    new_backward_input((3, 3), vec![0.; 9]),
                )
            };
            let node = MatrixMatrixMulBackward::new(lhs_f, lhs_b.clone(), rhs_f, rhs_b.clone());
            *node.gradient_mut() = new_tensor((3, 3), vec![1.; 9]);
            // -------------------------------------- Seed Gradient --------------------------------------
            assert_almost_equals(&*node.gradient(), &new_tensor((3, 3), vec![1.; 9]));
            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(
                &*lhs_b.gradient(),
                &new_tensor((3, 3), vec![33., 42., 51., 33., 42., 51., 33., 42., 51.]),
            );
            assert_almost_equals(
                &*rhs_b.gradient(),
                &new_tensor((3, 3), vec![12., 12., 12., 15., 15., 15., 18., 18., 18.]),
            );
            // ------------------------------------- Second Evaluation -------------------------------------
            lhs_b.set_overwrite(true);
            rhs_b.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(
                &*lhs_b.gradient(),
                &new_tensor((3, 3), vec![33., 42., 51., 33., 42., 51., 33., 42., 51.]),
            );
            assert_almost_equals(
                &*rhs_b.gradient(),
                &new_tensor((3, 3), vec![12., 12., 12., 15., 15., 15., 18., 18., 18.]),
            );
        }

        #[test]
        fn backward_left() {
            let (input_diff, input_not_diff) = {
                (
                    new_backward_input((3, 3), vec![0.; 9]),
                    new_input((3, 3), vec![10., 11., 12., 13., 14., 15., 16., 17., 18.]),
                )
            };
            let node = MatrixMatrixMulBackwardLeft::new(input_diff.clone(), input_not_diff);
            *node.gradient_mut() = new_tensor((3, 3), vec![1.; 9]);
            // -------------------------------------- Seed Gradient --------------------------------------
            assert_almost_equals(&*node.gradient(), &new_tensor((3, 3), vec![1.; 9]));
            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(
                &*input_diff.gradient(),
                &new_tensor((3, 3), vec![33., 42., 51., 33., 42., 51., 33., 42., 51.]),
            );
            // ------------------------------------- Second Evaluation -------------------------------------
            input_diff.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(
                &*input_diff.gradient(),
                &new_tensor((3, 3), vec![33., 42., 51., 33., 42., 51., 33., 42., 51.]),
            );
        }

        #[test]
        fn backward_right() {
            let (input_diff, input_not_diff) = {
                (
                    new_backward_input((3, 3), vec![0.; 9]),
                    new_input((3, 3), vec![1., 2., 3., 4., 5., 6., 7., 8., 9.]),
                )
            };
            let node = MatrixMatrixMulBackwardRight::new(input_not_diff, input_diff.clone());
            *node.gradient_mut() = new_tensor((3, 3), vec![1.; 9]);
            // -------------------------------------- Seed Gradient --------------------------------------
            assert_almost_equals(&*node.gradient(), &new_tensor((3, 3), vec![1.; 9]));
            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(
                &*input_diff.gradient(),
                &new_tensor((3, 3), vec![12., 12., 12., 15., 15., 15., 18., 18., 18.]),
            );
            // ------------------------------------- Second Evaluation -------------------------------------
            input_diff.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(
                &*input_diff.gradient(),
                &new_tensor((3, 3), vec![12., 12., 12., 15., 15., 15., 18., 18., 18.]),
            );
        }
    }

    mod backward_matrix_vector_mul {
        use super::*;
        #[test]
        fn backward() {
            let (lhs_f, lhs_b, rhs_f, rhs_b) = {
                (
                    new_input((3, 3), vec![1., 2., 3., 4., 5., 6., 7., 8., 9.]),
                    new_backward_input((3, 3), vec![0.; 9]),
                    new_input(3, vec![1., 2., 3.]),
                    new_backward_input(3, vec![0.; 3]),
                )
            };
            let node = MatrixVectorMulBackward::new(lhs_f, lhs_b.clone(), rhs_f, rhs_b.clone());
            *node.gradient_mut() = new_tensor(3, vec![1.; 3]);
            // -------------------------------------- Seed Gradient --------------------------------------
            assert_almost_equals(&*node.gradient(), &new_tensor(3, vec![1.; 3]));
            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(
                &*lhs_b.gradient(),
                &new_tensor((3, 3), vec![1., 2., 3., 1., 2., 3., 1., 2., 3.]),
            );
            assert_almost_equals(&*rhs_b.gradient(), &new_tensor(3, vec![12., 15., 18.]));
            // ------------------------------------- Second Evaluation -------------------------------------
            lhs_b.set_overwrite(true);
            rhs_b.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(
                &*lhs_b.gradient(),
                &new_tensor((3, 3), vec![1., 2., 3., 1., 2., 3., 1., 2., 3.]),
            );
            assert_almost_equals(&*rhs_b.gradient(), &new_tensor(3, vec![12., 15., 18.]));
        }

        #[test]
        fn backward_left() {
            let (input_diff, input_not_diff) = {
                (
                    new_backward_input((3, 3), vec![0.; 9]),
                    new_input(3, vec![1., 2., 3.]),
                )
            };
            let node = MatrixVectorMulBackwardLeft::new(input_diff.clone(), input_not_diff);
            *node.gradient_mut() = new_tensor(3, vec![1.; 3]);
            // -------------------------------------- Seed Gradient --------------------------------------
            assert_almost_equals(&*node.gradient(), &new_tensor(3, vec![1.; 3]));
            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(
                &*input_diff.gradient(),
                &new_tensor((3, 3), vec![1., 2., 3., 1., 2., 3., 1., 2., 3.]),
            );
            // ------------------------------------- Second Evaluation -------------------------------------
            input_diff.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(
                &*input_diff.gradient(),
                &new_tensor((3, 3), vec![1., 2., 3., 1., 2., 3., 1., 2., 3.]),
            );
        }

        #[test]
        fn backward_right() {
            let (input_diff, input_not_diff) = {
                (
                    new_backward_input(3, vec![0.; 3]),
                    new_input((3, 3), vec![1., 2., 3., 4., 5., 6., 7., 8., 9.]),
                )
            };
            let node = MatrixVectorMulBackwardRight::new(input_not_diff, input_diff.clone());
            *node.gradient_mut() = new_tensor(3, vec![1.; 3]);
            // -------------------------------------- Seed Gradient --------------------------------------
            assert_almost_equals(&*node.gradient(), &new_tensor(3, vec![1.; 3]));
            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(&*input_diff.gradient(), &new_tensor(3, vec![12., 15., 18.]));
            // ------------------------------------- Second Evaluation -------------------------------------
            input_diff.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(&*input_diff.gradient(), &new_tensor(3, vec![12., 15., 18.]));
        }
    }

    mod backward_vector_matrix_mul {
        use super::*;
        #[test]
        fn backward() {
            let (lhs_f, lhs_b, rhs_f, rhs_b) = {
                (
                    new_input(3, vec![1., 2., 3.]),
                    new_backward_input(3, vec![0.; 3]),
                    new_input((3, 3), vec![1., 2., 3., 4., 5., 6., 7., 8., 9.]),
                    new_backward_input((3, 3), vec![0.; 9]),
                )
            };
            let node = VectorMatrixMulBackward::new(lhs_f, lhs_b.clone(), rhs_f, rhs_b.clone());
            *node.gradient_mut() = new_tensor(3, vec![1.; 3]);
            // -------------------------------------- Seed Gradient --------------------------------------
            assert_almost_equals(&*node.gradient(), &new_tensor(3, vec![1.; 3]));
            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(&*lhs_b.gradient(), &new_tensor(3, vec![6., 15., 24.]));
            assert_almost_equals(
                &*rhs_b.gradient(),
                &new_tensor((3, 3), vec![1., 1., 1., 2., 2., 2., 3., 3., 3.]),
            );
            // ------------------------------------- Second Evaluation -------------------------------------
            lhs_b.set_overwrite(true);
            rhs_b.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(&*lhs_b.gradient(), &new_tensor(3, vec![6., 15., 24.]));
            assert_almost_equals(
                &*rhs_b.gradient(),
                &new_tensor((3, 3), vec![1., 1., 1., 2., 2., 2., 3., 3., 3.]),
            );
        }

        #[test]
        fn backward_left() {
            let (input_diff, input_not_diff) = {
                (
                    new_backward_input(3, vec![0.; 3]),
                    new_input((3, 3), vec![1., 2., 3., 4., 5., 6., 7., 8., 9.]),
                )
            };
            let node = VectorMatrixMulBackwardLeft::new(input_diff.clone(), input_not_diff);
            *node.gradient_mut() = new_tensor(3, vec![1.; 3]);
            // -------------------------------------- Seed Gradient --------------------------------------
            assert_almost_equals(&*node.gradient(), &new_tensor(3, vec![1.; 3]));
            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(&*input_diff.gradient(), &new_tensor(3, vec![6., 15., 24.]));
            // ------------------------------------- Second Evaluation -------------------------------------
            input_diff.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(&*input_diff.gradient(), &new_tensor(3, vec![6., 15., 24.]));
        }

        #[test]
        fn backward_right() {
            let (input_diff, input_not_diff) = {
                (
                    new_backward_input((3, 3), vec![0.; 9]),
                    new_input(3, vec![1., 2., 3.]),
                )
            };
            let node = VectorMatrixMulBackwardRight::new(input_not_diff, input_diff.clone());
            *node.gradient_mut() = new_tensor(3, vec![1.; 3]);
            // -------------------------------------- Seed Gradient --------------------------------------
            assert_almost_equals(&*node.gradient(), &new_tensor(3, vec![1.; 3]));
            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(
                &*input_diff.gradient(),
                &new_tensor((3, 3), vec![1., 1., 1., 2., 2., 2., 3., 3., 3.]),
            );
            // ------------------------------------- Second Evaluation -------------------------------------
            input_diff.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(
                &*input_diff.gradient(),
                &new_tensor((3, 3), vec![1., 1., 1., 2., 2., 2., 3., 3., 3.]),
            );
        }
    }

    mod backward_vector_vector_mul {
        use super::*;
        #[test]
        fn backward() {
            let (lhs_f, lhs_b, rhs_f, rhs_b) = {
                (
                    new_input(3, vec![1., 2., 3.]),
                    new_backward_input(3, vec![0.; 3]),
                    new_input(3, vec![4., 5., 6.]),
                    new_backward_input(3, vec![0.; 3]),
                )
            };
            let node = VectorVectorMulBackward::new(lhs_f, lhs_b.clone(), rhs_f, rhs_b.clone());
            *node.gradient_mut() = new_tensor(3, vec![1.; 3]);
            // -------------------------------------- Seed Gradient --------------------------------------
            assert_almost_equals(&*node.gradient(), &new_tensor(3, vec![1.; 3]));
            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(&*lhs_b.gradient(), &new_tensor(3, vec![4., 5., 6.]));
            assert_almost_equals(&*rhs_b.gradient(), &new_tensor(3, vec![1., 2., 3.]));
            // ------------------------------------- Second Evaluation -------------------------------------
            lhs_b.set_overwrite(true);
            rhs_b.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(&*lhs_b.gradient(), &new_tensor(3, vec![4., 5., 6.]));
            assert_almost_equals(&*rhs_b.gradient(), &new_tensor(3, vec![1., 2., 3.]));
        }

        #[test]
        fn backward_unary() {
            let (input_diff, input_not_diff) = {
                (
                    new_backward_input(3, vec![0.; 3]),
                    new_input(3, vec![1., 2., 3.]),
                )
            };
            let node = VectorVectorMulBackwardUnary::new(input_diff.clone(), input_not_diff);
            *node.gradient_mut() = new_tensor(3, vec![1.; 3]);
            // -------------------------------------- Seed Gradient --------------------------------------
            assert_almost_equals(&*node.gradient(), &new_tensor(3, vec![1.; 3]));
            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(&*input_diff.gradient(), &new_tensor(3, vec![1., 2., 3.]));
            // ------------------------------------- Second Evaluation -------------------------------------
            input_diff.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(&*input_diff.gradient(), &new_tensor(3, vec![1., 2., 3.]));
        }
    }

    mod backward_power {
        use super::*;
        #[test]
        fn backward() {
            let (input, input_diff, exp) = (
                new_input(3, vec![1., 2., 3.]),
                new_backward_input(3, vec![0.; 3]),
                3,
            );
            let node = PowerBackward::new(input_diff.clone(), input, exp);

            // -------------------------------------- Seed Gradient --------------------------------------
            *node.gradient_mut() = new_tensor(3, vec![1.; 3]);
            assert_almost_equals(&*node.gradient(), &new_tensor(3, vec![1.; 3]));

            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(&*input_diff.gradient(), &new_tensor(3, vec![3., 12., 27.]));

            // ------------------------------------- Second Evaluation -------------------------------------
            input_diff.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(&*input_diff.gradient(), &new_tensor(3, vec![3., 12., 27.]));
        }
        #[test]
        fn backward_negative_exp() {
            let (input, input_diff, exp) = (
                new_input(3, vec![1., 2., 3.]),
                new_backward_input(3, vec![0.; 3]),
                -3,
            );
            let node = PowerBackward::new(input_diff.clone(), input, exp);

            // -------------------------------------- Seed Gradient --------------------------------------
            *node.gradient_mut() = new_tensor(3, vec![1.; 3]);
            assert_almost_equals(&*node.gradient(), &new_tensor(3, vec![1.; 3]));

            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(
                &*input_diff.gradient(),
                &new_tensor(3, vec![-3.0000, -0.1875, -0.037037037]),
            );

            // ------------------------------------- Second Evaluation -------------------------------------
            input_diff.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(
                &*input_diff.gradient(),
                &new_tensor(3, vec![-3.0000, -0.1875, -0.037037037]),
            );
        }
    }

    mod backward_sum {
        use super::*;
        #[test]
        fn backward() {
            let input_diff = new_backward_input((10, 10), vec![0.; 100]);
            let node = SumBackward::new(input_diff.clone());

            // -------------------------------------- Seed Gradient --------------------------------------
            *node.gradient_mut() = new_tensor(1, vec![1.]);
            assert_almost_equals(&*node.gradient(), &new_tensor(1, vec![1.]));

            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(
                &*input_diff.gradient(),
                &new_tensor((10, 10), vec![1.; 100]),
            );

            // ------------------------------------- Second Evaluation -------------------------------------
            input_diff.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(
                &*input_diff.gradient(),
                &new_tensor((10, 10), vec![1.; 100]),
            );
        }
    }

    mod backward_logn {
        use super::*;

        #[test]
        fn backward() {
            let (input, input_diff) = (
                new_input(3, vec![1., 2., 3.]),
                new_backward_input(3, vec![0.; 3]),
            );
            let node = LognBackward::new(input_diff.clone(), input);

            // -------------------------------------- Seed Gradient --------------------------------------
            *node.gradient_mut() = new_tensor(3, vec![1.; 3]);
            assert_almost_equals(&*node.gradient(), &new_tensor(3, vec![1.; 3]));

            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(
                &*input_diff.gradient(),
                &new_tensor(3, vec![1., 0.5, 0.33333]),
            );

            // ------------------------------------- Second Evaluation -------------------------------------
            input_diff.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(
                &*input_diff.gradient(),
                &new_tensor(3, vec![1., 0.5, 0.33333]),
            );
        }
    }

    mod backward_relu {
        use super::*;
        #[test]
        fn backward() {
            let (input, input_diff) = (
                new_input(3, vec![-1., 2., -3.]),
                new_backward_input(3, vec![0.; 3]),
            );
            let node = ReLUBackward::new(input_diff.clone(), input);

            // -------------------------------------- Seed Gradient --------------------------------------
            *node.gradient_mut() = new_tensor(3, vec![1.; 3]);
            assert_almost_equals(&*node.gradient(), &new_tensor(3, vec![1.; 3]));

            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(&*input_diff.gradient(), &new_tensor(3, vec![0., 1., 0.]));

            // ------------------------------------- Second Evaluation -------------------------------------
            input_diff.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(&*input_diff.gradient(), &new_tensor(3, vec![0., 1., 0.]));
        }
    }

    mod backward_leaky_relu {
        use super::*;
        #[test]
        fn backward() {
            let (input, input_diff) = (
                new_input(3, vec![-1., 2., -3.]),
                new_backward_input(3, vec![0.; 3]),
            );
            let node = LeakyReLUBackward::new(input_diff.clone(), input);

            // -------------------------------------- Seed Gradient --------------------------------------
            *node.gradient_mut() = new_tensor(3, vec![1.; 3]);
            assert_almost_equals(&*node.gradient(), &new_tensor(3, vec![1.; 3]));

            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(
                &*input_diff.gradient(),
                &new_tensor(3, vec![0.01, 1., 0.01]),
            );

            // ------------------------------------- Second Evaluation -------------------------------------
            input_diff.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(
                &*input_diff.gradient(),
                &new_tensor(3, vec![0.01, 1., 0.01]),
            );
        }
    }

    mod backward_softplus {
        use super::*;
        #[test]
        fn backward() {
            let (input, input_diff) = (
                new_input(3, vec![1., 2., 3.]),
                new_backward_input(3, vec![0.; 3]),
            );
            let node = SoftPlusBackward::new(input_diff.clone(), input);

            // -------------------------------------- Seed Gradient --------------------------------------
            *node.gradient_mut() = new_tensor(3, vec![1.; 3]);
            assert_almost_equals(&*node.gradient(), &new_tensor(3, vec![1.; 3]));

            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(
                &*input_diff.gradient(),
                &new_tensor(3, vec![0.7311, 0.8808, 0.9526]),
            );

            // ------------------------------------- Second Evaluation -------------------------------------
            input_diff.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(
                &*input_diff.gradient(),
                &new_tensor(3, vec![0.7311, 0.8808, 0.9526]),
            );
        }
    }

    mod backward_sigmoid {
        use super::*;
        use crate::variable::node::{Forward, Sigmoid};
        #[test]
        fn backward() {
            let (input, input_diff) = (
                new_input(3, vec![1., 2., 3.]),
                new_backward_input(3, vec![0.; 3]),
            );
            let node_f = Sigmoid::new(input);
            node_f.forward();
            let node_b = SigmoidBackward::new(input_diff.clone(), Rc::new(node_f));

            // -------------------------------------- Seed Gradient --------------------------------------
            *node_b.gradient_mut() = new_tensor(3, vec![1.; 3]);
            assert_almost_equals(&*node_b.gradient(), &new_tensor(3, vec![1.; 3]));

            // ------------------------------------- First Evaluation -------------------------------------
            node_b.backward();
            assert_almost_equals(
                &*input_diff.gradient(),
                &new_tensor(3, vec![0.1966, 0.1050, 0.0452]),
            );

            // ------------------------------------- Second Evaluation -------------------------------------
            input_diff.set_overwrite(true);
            node_b.set_overwrite(true);
            node_b.backward();
            assert_almost_equals(
                &*input_diff.gradient(),
                &new_tensor(3, vec![0.1966, 0.1050, 0.0452]),
            );
        }
    }

    mod backward_tanh {
        use super::*;
        use crate::variable::node::{Forward, TanH};
        #[test]
        fn backward() {
            let (input, input_diff) = (
                new_input(3, vec![1., 2., 3.]),
                new_backward_input(3, vec![0.; 3]),
            );
            let node_f = TanH::new(input);
            node_f.forward();
            let node_b = TanHBackward::new(input_diff.clone(), Rc::new(node_f));

            // -------------------------------------- Seed Gradient --------------------------------------
            *node_b.gradient_mut() = new_tensor(3, vec![1.; 3]);
            assert_almost_equals(&*node_b.gradient(), &new_tensor(3, vec![1.; 3]));

            // ------------------------------------- First Evaluation -------------------------------------
            node_b.backward();
            assert_almost_equals(
                &*input_diff.gradient(),
                &new_tensor(3, vec![0.4199, 0.07065, 0.009865]),
            );

            // ------------------------------------- Second Evaluation -------------------------------------
            input_diff.set_overwrite(true);
            node_b.set_overwrite(true);
            node_b.backward();
            assert_almost_equals(
                &*input_diff.gradient(),
                &new_tensor(3, vec![0.4199, 0.07065, 0.009865]),
            );
        }
    }

    mod backward_exp {
        use super::*;
        use crate::variable::node::{Exp, Forward};

        #[allow(clippy::clippy::approx_constant)]
        #[test]
        fn backward() {
            let (input, input_diff) = (
                new_input(3, vec![1., 2., 3.]),
                new_backward_input(3, vec![0.; 3]),
            );
            let node_f = Exp::new(input);
            node_f.forward();
            let node_b = ExpBackward::new(input_diff.clone(), Rc::new(node_f));

            // -------------------------------------- Seed Gradient --------------------------------------
            *node_b.gradient_mut() = new_tensor(3, vec![1.; 3]);
            assert_almost_equals(&*node_b.gradient(), &new_tensor(3, vec![1.; 3]));

            // ------------------------------------- First Evaluation -------------------------------------
            node_b.backward();
            assert_almost_equals(
                &*input_diff.gradient(),
                &new_tensor(3, vec![2.7183, 7.3891, 20.0855]),
            );

            // ------------------------------------- Second Evaluation -------------------------------------
            input_diff.set_overwrite(true);
            node_b.set_overwrite(true);
            node_b.backward();
            assert_almost_equals(
                &*input_diff.gradient(),
                &new_tensor(3, vec![2.7183, 7.3891, 20.0855]),
            );
        }
    }

    mod backward_softmax {
        use super::*;
        use crate::variable::node::{Forward, Softmax};

        #[test]
        fn backward() {
            let (input, input_diff, axis) = (
                new_input((3, 3), vec![1., 2., 3., 4., 5., 6., 7., 8., 9.]),
                new_backward_input((3, 3), vec![0.; 9]),
                0,
            );
            let node_f = Softmax::new(input, axis);
            node_f.forward();
            let node_b = SoftmaxBackward::new(input_diff.clone(), Rc::new(node_f), axis);

            // -------------------------------------- Seed Gradient --------------------------------------
            *node_b.gradient_mut() = new_tensor((3, 3), vec![1., 2., 3., 4., 5., 6., 7., 8., 9.]);
            assert_almost_equals(
                &*node_b.gradient(),
                &new_tensor((3, 3), vec![1., 2., 3., 4., 5., 6., 7., 8., 9.]),
            );

            // ------------------------------------- First Evaluation -------------------------------------
            node_b.backward();
            assert_almost_equals(
                &*input_diff.gradient(),
                &new_tensor(
                    (3, 3),
                    vec![
                        -0.01376, -0.01376, -0.01376, -0.13455, -0.13455, -0.13455, 0.148323,
                        0.148323, 0.148323,
                    ],
                ),
            );

            // ------------------------------------- Second Evaluation -------------------------------------
            input_diff.set_overwrite(true);
            node_b.set_overwrite(true);
            node_b.backward();
            assert_almost_equals(
                &*input_diff.gradient(),
                &new_tensor(
                    (3, 3),
                    vec![
                        -0.01376, -0.01376, -0.01376, -0.13455, -0.13455, -0.13455, 0.148323,
                        0.148323, 0.148323,
                    ],
                ),
            );
        }
    }

    mod backward_logsoftmax {
        use super::*;
        use crate::variable::node::{Forward, LogSoftmax};

        #[test]
        fn backward() {
            let (input, input_diff, axis) = (
                new_input((3, 3), vec![1., 2., 3., 4., 5., 6., 7., 8., 9.]),
                new_backward_input((3, 3), vec![0.; 9]),
                0,
            );
            let node_f = LogSoftmax::new(input, axis);
            node_f.forward();
            let node_b = LogSoftmaxBackward::new(input_diff.clone(), Rc::new(node_f), axis);

            // -------------------------------------- Seed Gradient --------------------------------------
            *node_b.gradient_mut() = new_tensor((3, 3), vec![1., 2., 3., 4., 5., 6., 7., 8., 9.]);
            assert_almost_equals(
                &*node_b.gradient(),
                &new_tensor((3, 3), vec![1., 2., 3., 4., 5., 6., 7., 8., 9.]),
            );

            // ------------------------------------- First Evaluation -------------------------------------
            node_b.backward();
            assert_almost_equals(
                &*input_diff.gradient(),
                &new_tensor(
                    (3, 3),
                    vec![
                        0.9717, 1.9647, 2.9576, 3.4322, 4.2903, 5.1483, -4.4040, -6.2550, -8.1059,
                    ],
                ),
            );

            // ------------------------------------- Second Evaluation -------------------------------------
            input_diff.set_overwrite(true);
            node_b.set_overwrite(true);
            node_b.backward();
            assert_almost_equals(
                &*input_diff.gradient(),
                &new_tensor(
                    (3, 3),
                    vec![
                        0.9717, 1.9647, 2.9576, 3.4322, 4.2903, 5.1483, -4.4040, -6.2550, -8.1059,
                    ],
                ),
            );
        }
    }

    mod backward_transpose {
        use super::*;
        #[test]
        fn backward() {
            let input = new_backward_input((4, 3), vec![0.; 12]);
            let node = TransposeBackward::new(input.clone());

            // -------------------------------------- Seed Gradient --------------------------------------
            *node.gradient_mut() = new_tensor((3, 4), vec![1.; 12]);
            assert_almost_equals(&*node.gradient(), &new_tensor((3, 4), vec![1.; 12]));

            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(&*input.gradient(), &new_tensor((4, 3), vec![1.; 12]));

            // ------------------------------------- Second Evaluation -------------------------------------
            input.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(&*input.gradient(), &new_tensor((4, 3), vec![1.; 12]));
        }
    }

    mod backward_chunks {
        use super::*;
        #[test]
        fn backward() {
            let input = new_backward_input((4, 3), vec![0.; 12]);

            let chunk_0 = ChunkBackward::new(input.clone(), Tensor::zeros((1, 3)), 0);
            let chunk_1 = ChunkBackward::new(input.clone(), Tensor::zeros((1, 3)), 1);
            let chunk_2 = ChunkBackward::new(input.clone(), Tensor::zeros((1, 3)), 2);
            let chunk_3 = ChunkBackward::new(input.clone(), Tensor::zeros((1, 3)), 3);

            // -------------------------------------- Seed Gradient of each Chunks  --------------------------------------
            *chunk_0.gradient_mut() = new_tensor((1, 3), vec![1.; 3]);
            *chunk_1.gradient_mut() = new_tensor((1, 3), vec![2.; 3]);
            *chunk_2.gradient_mut() = new_tensor((1, 3), vec![3.; 3]);
            *chunk_3.gradient_mut() = new_tensor((1, 3), vec![4.; 3]);

            // ------------------------------------- First Evaluation -------------------------------------
            chunk_0.backward();
            chunk_1.backward();
            chunk_2.backward();
            chunk_3.backward();
            assert_almost_equals(
                &*input.gradient(),
                &new_tensor((4, 3), vec![1., 1., 1., 2., 2., 2., 3., 3., 3., 4., 4., 4.]),
            );

            // ------------------------------------- Second Evaluation -------------------------------------
            input.set_overwrite(true);
            input.gradient_mut().map_inplace(|el| *el = 0.);
            chunk_0.set_overwrite(true);
            chunk_1.set_overwrite(true);
            chunk_2.set_overwrite(true);
            chunk_3.set_overwrite(true);
            chunk_0.backward();
            chunk_1.backward();
            chunk_2.backward();
            chunk_3.backward();
            assert_almost_equals(
                &*input.gradient(),
                &new_tensor((4, 3), vec![1., 1., 1., 2., 2., 2., 3., 3., 3., 4., 4., 4.]),
            );
        }
    }

    mod backward_unsqueeze {
        use super::*;
        #[test]
        fn backward() {
            let input = new_backward_input((4, 3), vec![0.; 12]);
            let node = UnsqueezeBackward::new(input.clone(), 0);

            // -------------------------------------- Seed Gradient --------------------------------------
            *node.gradient_mut() = new_tensor((1, 4, 3), vec![1.; 12]);
            assert_almost_equals(&*node.gradient(), &new_tensor((1, 4, 3), vec![1.; 12]));

            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(&*input.gradient(), &new_tensor((4, 3), vec![1.; 12]));

            // ------------------------------------- Second Evaluation -------------------------------------
            input.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(&*input.gradient(), &new_tensor((4, 3), vec![1.; 12]));
        }
    }

    mod backward_cat {
        use super::*;
        #[test]
        fn backward() {
            let lhs = new_backward_input((4, 3), vec![0.; 12]);
            let rhs = new_backward_input((4, 2), vec![0.; 8]);

            let node = ConcatenateBackward::new(lhs.clone(), rhs.clone(), 1);

            // -------------------------------------- Seed Gradient --------------------------------------
            *node.gradient_mut() = new_tensor((4, 5), vec![1.; 20]);
            assert_almost_equals(&*node.gradient(), &new_tensor((4, 5), vec![1.; 20]));

            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(&*lhs.gradient(), &new_tensor((4, 3), vec![1.; 12]));
            assert_almost_equals(&*rhs.gradient(), &new_tensor((4, 2), vec![1.; 8]));

            // ------------------------------------- Second Evaluation -------------------------------------
            lhs.set_overwrite(true);
            rhs.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(&*lhs.gradient(), &new_tensor((4, 3), vec![1.; 12]));
            assert_almost_equals(&*rhs.gradient(), &new_tensor((4, 2), vec![1.; 8]));
        }
        #[test]
        fn backward_left() {
            let lhs = new_backward_input((4, 3), vec![0.; 12]);
            let rhs = new_input((4, 2), vec![0.; 8]);

            let node = ConcatenateBackwardLeft::new(lhs.clone(), rhs, 1);

            // -------------------------------------- Seed Gradient --------------------------------------
            *node.gradient_mut() = new_tensor((4, 5), vec![1.; 20]);
            assert_almost_equals(&*node.gradient(), &new_tensor((4, 5), vec![1.; 20]));

            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(&*lhs.gradient(), &new_tensor((4, 3), vec![1.; 12]));

            // ------------------------------------- Second Evaluation -------------------------------------
            lhs.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(&*lhs.gradient(), &new_tensor((4, 3), vec![1.; 12]));
        }
        #[test]
        fn backward_right() {
            let lhs = new_input((4, 3), vec![0.; 12]);
            let rhs = new_backward_input((4, 2), vec![0.; 8]);

            let node = ConcatenateBackwardRight::new(lhs, rhs.clone(), 1);

            // -------------------------------------- Seed Gradient --------------------------------------
            *node.gradient_mut() = new_tensor((4, 5), vec![1.; 20]);
            assert_almost_equals(&*node.gradient(), &new_tensor((4, 5), vec![1.; 20]));

            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(&*rhs.gradient(), &new_tensor((4, 2), vec![1.; 8]));

            // ------------------------------------- Second Evaluation -------------------------------------
            rhs.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(&*rhs.gradient(), &new_tensor((4, 2), vec![1.; 8]));
        }
    }

    mod backward_stack {
        use super::*;
        #[test]
        fn backward() {
            let lhs = new_backward_input((4, 3), vec![0.; 12]);
            let rhs = new_backward_input((4, 3), vec![0.; 12]);

            let node = StackBackward::new(lhs.clone(), rhs.clone(), 0);

            // -------------------------------------- Seed Gradient --------------------------------------
            *node.gradient_mut() = new_tensor((2, 4, 3), vec![1.; 24]);
            assert_almost_equals(&*node.gradient(), &new_tensor((2, 4, 3), vec![1.; 24]));

            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(&*lhs.gradient(), &new_tensor((4, 3), vec![1.; 12]));
            assert_almost_equals(&*rhs.gradient(), &new_tensor((4, 3), vec![1.; 12]));

            // ------------------------------------- Second Evaluation -------------------------------------
            lhs.set_overwrite(true);
            rhs.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(&*lhs.gradient(), &new_tensor((4, 3), vec![1.; 12]));
            assert_almost_equals(&*rhs.gradient(), &new_tensor((4, 3), vec![1.; 12]));
        }
        #[test]
        fn backward_left() {
            let lhs = new_backward_input((4, 3), vec![0.; 12]);
            let rhs = new_input((4, 3), vec![0.; 12]);

            let node = StackBackwardLeft::new(lhs.clone(), rhs, 0);

            // -------------------------------------- Seed Gradient --------------------------------------
            *node.gradient_mut() = new_tensor((2, 4, 3), vec![1.; 24]);
            assert_almost_equals(&*node.gradient(), &new_tensor((2, 4, 3), vec![1.; 24]));

            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(&*lhs.gradient(), &new_tensor((4, 3), vec![1.; 12]));

            // ------------------------------------- Second Evaluation -------------------------------------
            lhs.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(&*lhs.gradient(), &new_tensor((4, 3), vec![1.; 12]));
        }

        #[test]
        fn backward_right() {
            let lhs = new_input((4, 3), vec![0.; 12]);
            let rhs = new_backward_input((4, 3), vec![0.; 12]);

            let node = StackBackwardRight::new(lhs, rhs.clone(), 0);

            // -------------------------------------- Seed Gradient --------------------------------------
            *node.gradient_mut() = new_tensor((2, 4, 3), vec![1.; 24]);
            assert_almost_equals(&*node.gradient(), &new_tensor((2, 4, 3), vec![1.; 24]));

            // ------------------------------------- First Evaluation -------------------------------------
            node.backward();
            assert_almost_equals(&*rhs.gradient(), &new_tensor((4, 3), vec![1.; 12]));

            // ------------------------------------- Second Evaluation -------------------------------------
            rhs.set_overwrite(true);
            node.set_overwrite(true);
            node.backward();
            assert_almost_equals(&*rhs.gradient(), &new_tensor((4, 3), vec![1.; 12]));
        }
    }
}