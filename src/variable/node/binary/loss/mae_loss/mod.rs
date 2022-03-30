use super::{reduction::Reduction, Backward, Forward, OptionalTensor, Tensor};
use ndarray::{arr0, Dimension, Ix0, Zip};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

#[allow(clippy::upper_case_acronyms)]
pub struct MAELoss<D>
where
    D: Dimension,
{
    input_data: Rc<RefCell<Tensor<D>>>,
    target_data: Rc<RefCell<Tensor<D>>>,
    data: Rc<RefCell<Tensor<Ix0>>>,
    reduction: Reduction,
    computed: Cell<bool>,
}

impl<D> MAELoss<D>
where
    D: Dimension,
{
    pub(crate) fn new(
        input_data: Rc<RefCell<Tensor<D>>>,
        target_data: Rc<RefCell<Tensor<D>>>,
        data: Rc<RefCell<Tensor<Ix0>>>,
        reduction: Reduction,
    ) -> Self {
        Self {
            input_data,
            target_data,
            data,
            reduction,
            computed: Cell::default(),
        }
    }
}

impl<D> Forward for MAELoss<D>
where
    D: Dimension,
{
    fn forward(&self) {
        if self.was_computed() {
            return;
        }

        self.computed.set(true);
        let (mut data, input_data, target_data) = {
            (
                self.data.borrow_mut(),
                self.input_data.borrow(),
                self.target_data.borrow(),
            )
        };

        *data = {
            let total_loss = Zip::from(&*input_data)
                .and(&*target_data)
                .fold(0.0, |loss, input, target| loss + (input - target).abs());
            match self.reduction {
                Reduction::Mean => arr0(total_loss / input_data.len() as f32),
                Reduction::Sum => arr0(total_loss),
            }
        };
    }

    fn was_computed(&self) -> bool {
        self.computed.get()
    }

    fn reset_computation(&self) {
        self.computed.set(false);
    }
}

#[allow(clippy::upper_case_acronyms)]
pub struct MAELossBackward<D>
where
    D: Dimension,
{
    input_data: Rc<RefCell<Tensor<D>>>,
    target_data: Rc<RefCell<Tensor<D>>>,
    input_gradient: Rc<OptionalTensor<D>>,
    gradient: Rc<OptionalTensor<Ix0>>,
    reduction: Reduction,
}

impl<D> MAELossBackward<D>
where
    D: Dimension,
{
    pub(crate) fn new(
        input_data: Rc<RefCell<Tensor<D>>>,
        target_data: Rc<RefCell<Tensor<D>>>,
        input_gradient: Rc<OptionalTensor<D>>,
        gradient: Rc<OptionalTensor<Ix0>>,
        reduction: Reduction,
    ) -> Self {
        Self {
            input_data,
            target_data,
            input_gradient,
            gradient,
            reduction,
        }
    }
}

impl<D> Backward for MAELossBackward<D>
where
    D: Dimension,
{
    fn backward(&self) {
        let mut input_gradient = self.input_gradient.content_mut();
        let gradient = self.gradient.content();
        let input_data = self.input_data.borrow();
        let target_data = self.target_data.borrow();
        let zip = Zip::from(&mut *input_gradient)
            .and_broadcast(&*gradient)
            .and(&*input_data)
            .and(&*target_data);

        match self.reduction {
            Reduction::Mean => {
                let n = input_data.len() as f32;
                zip.for_each(|op_grad, &grad, &input, &target| {
                    let diff = input - target;
                    *op_grad += ((diff != 0.0) as u8 as f32) * (diff.signum() * grad / n);
                });
            }
            Reduction::Sum => {
                zip.for_each(|op_grad, &grad, &input, &target| {
                    let diff = input - target;
                    *op_grad += ((diff != 0.0) as u8 as f32) * (diff.signum() * grad)
                });
            }
        }
    }
}

// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~ Tests ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
//#[cfg(test)]
//mod test;
