use ndarray::{Array, Dimension};
use std::{
    cell::{Ref, RefCell},
    error::Error,
    rc::Rc,
};

fn new_shared<T>(item: T) -> Rc<RefCell<T>> {
    Rc::new(RefCell::new(item))
}

fn are_similar<D: Dimension>(
    result: Ref<Array<f32, D>>,
    expected: &Array<f32, D>,
) -> Result<(), Box<dyn Error>> {
    if !result.abs_diff_eq(expected, f32::EPSILON) {
        return Err(format!("Result: {} | Expected: {}", result, expected).into());
    }

    Ok(())
}

mod forward {
    use super::super::{Addition, Forward};
    use super::*;

    #[test]
    fn creation() -> Result<(), Box<dyn Error>> {
        let left = Array::linspace(1., 9., 9).into_shape((3, 3))?;
        let right = Array::ones((3, 3));
        let data = Array::zeros((3, 3));
        let op = Addition::new(
            new_shared(left.clone()),
            new_shared(right.clone()),
            new_shared(data.clone()),
        );

        are_similar(op.left.borrow(), &left)?;
        are_similar(op.right.borrow(), &right)?;
        are_similar(op.data.borrow(), &data)
    }

    #[test]
    fn base_case() -> Result<(), Box<dyn Error>> {
        let left = Array::linspace(1., 9., 9).into_shape((3, 3))?;
        let right = Array::zeros((3, 3));
        let data = Array::zeros((3, 3));
        let op = Addition::new(
            new_shared(left.clone()),
            new_shared(right.clone()),
            new_shared(data),
        );

        op.forward();
        are_similar(op.data.borrow(), &(left + right))
    }

    #[test]
    fn left_broadcast() -> Result<(), Box<dyn Error>> {
        let left = Array::linspace(1., 3., 3).into_shape((1, 3))?;
        let right = Array::ones((2, 2, 3));
        let data = Array::zeros((2, 2, 3));
        let op = Addition::new(
            new_shared(left.clone()),
            new_shared(right.clone()),
            new_shared(data),
        );

        op.forward();
        are_similar(op.data.borrow(), &(left + right))
    }

    #[test]
    #[should_panic]
    fn wrong_left_broadcast() {
        let left = Array::linspace(1., 3., 9).into_shape((3, 3)).unwrap();
        let right = Array::ones((2, 2, 3));
        let data = Array::zeros((2, 2, 3));
        let op = Addition::new(new_shared(left), new_shared(right), new_shared(data));

        op.forward();
    }

    #[test]
    fn right_broadcast() -> Result<(), Box<dyn Error>> {
        let left = Array::ones((2, 2, 3));
        let right = Array::linspace(1., 3., 3).into_shape((1, 3))?;
        let data = Array::zeros((2, 2, 3));
        let op = Addition::new(
            new_shared(left.clone()),
            new_shared(right.clone()),
            new_shared(data),
        );

        op.forward();
        are_similar(op.data.borrow(), &(left + right))
    }

    #[test]
    #[should_panic]
    fn wrong_right_broadcast() {
        let left = Array::ones((2, 2, 3));
        let right = Array::linspace(1., 3., 9).into_shape((3, 3)).unwrap();
        let data = Array::zeros((2, 2, 3));
        let op = Addition::new(new_shared(left), new_shared(right), new_shared(data));

        op.forward();
    }
}

mod backward {
    use super::super::{
        AdditionBackward, AdditionBackwardLeft, AdditionBackwardRight, Backward, Gradient,
    };
    use super::*;
    use ndarray::{Ix1, Ix2};

    #[test]
    fn left_creation() -> Result<(), Box<dyn Error>> {
        let left = Array::zeros((3, 3));
        let grad = Array::ones((3, 3));
        let op = AdditionBackwardLeft::<Ix2, Ix2>::new(
            Rc::new(Gradient::from_ndarray(left.clone())),
            Rc::new(Gradient::from_ndarray(grad.clone())),
        );

        are_similar(op.operand_gradient.borrow(), &left)?;
        are_similar(op.gradient.borrow(), &grad)
    }

    #[test]
    fn left_base_case() -> Result<(), Box<dyn Error>> {
        let left = Array::zeros((3, 3));
        let grad = Array::ones((3, 3));
        let op = AdditionBackwardLeft::<Ix2, Ix2>::new(
            Rc::new(Gradient::from_ndarray(left.clone())),
            Rc::new(Gradient::from_ndarray(grad.clone())),
        );

        op.backward();
        are_similar(op.operand_gradient.borrow(), &(&left + &grad))?;
        are_similar(op.gradient.borrow(), &grad)?;

        op.backward();
        are_similar(op.operand_gradient.borrow(), &(&left + &grad * 2.))?;
        are_similar(op.gradient.borrow(), &grad)
    }

    #[test]
    fn left_reduction() -> Result<(), Box<dyn Error>> {
        let left = Array::zeros(3);
        let grad = Array::ones((3, 3));
        let op = AdditionBackwardLeft::<Ix1, Ix2>::new(
            Rc::new(Gradient::from_ndarray(left)),
            Rc::new(Gradient::from_ndarray(grad)),
        );

        op.backward();
        are_similar(op.operand_gradient.borrow(), &Array::from_elem(3, 3.))?;
        are_similar(op.gradient.borrow(), &Array::ones((3, 3)))?;

        op.backward();
        are_similar(op.operand_gradient.borrow(), &Array::from_elem(3, 6.))?;
        are_similar(op.gradient.borrow(), &Array::ones((3, 3)))
    }

    #[test]
    #[should_panic]
    fn wrong_left_reduction() {
        let left = Array::zeros(2);
        let grad = Array::ones((3, 3));
        let op = AdditionBackwardLeft::<Ix1, Ix2>::new(
            Rc::new(Gradient::from_ndarray(left)),
            Rc::new(Gradient::from_ndarray(grad)),
        );

        op.backward();
    }

    #[test]
    fn right_creation() -> Result<(), Box<dyn Error>> {
        let right = Array::zeros((3, 3));
        let grad = Array::ones((3, 3));
        let op = AdditionBackwardRight::<Ix2, Ix2>::new(
            Rc::new(Gradient::from_ndarray(right.clone())),
            Rc::new(Gradient::from_ndarray(grad.clone())),
        );

        are_similar(op.operand_gradient.borrow(), &right)?;
        are_similar(op.gradient.borrow(), &grad)
    }

    #[test]
    fn right_base_case() -> Result<(), Box<dyn Error>> {
        let right = Array::zeros((3, 3));
        let grad = Array::ones((3, 3));
        let op = AdditionBackwardRight::<Ix2, Ix2>::new(
            Rc::new(Gradient::from_ndarray(right.clone())),
            Rc::new(Gradient::from_ndarray(grad.clone())),
        );

        op.backward();
        are_similar(op.operand_gradient.borrow(), &(&right + &grad))?;
        are_similar(op.gradient.borrow(), &grad)?;

        op.backward();
        are_similar(op.operand_gradient.borrow(), &(&right + &grad * 2.))?;
        are_similar(op.gradient.borrow(), &grad)
    }

    #[test]
    fn right_reduction() -> Result<(), Box<dyn Error>> {
        let right = Array::zeros(3);
        let grad = Array::ones((3, 3));
        let op = AdditionBackwardRight::<Ix2, Ix1>::new(
            Rc::new(Gradient::from_ndarray(right)),
            Rc::new(Gradient::from_ndarray(grad.clone())),
        );

        op.backward();
        are_similar(op.operand_gradient.borrow(), &Array::from_elem(3, 3.))?;
        are_similar(op.gradient.borrow(), &grad)?;

        op.backward();
        are_similar(op.operand_gradient.borrow(), &Array::from_elem(3, 6.))?;
        are_similar(op.gradient.borrow(), &grad)
    }

    #[test]
    #[should_panic]
    fn wrong_right_reduction() {
        let right = Array::zeros(2);
        let grad = Array::ones((3, 3));
        let op = AdditionBackwardRight::<Ix2, Ix1>::new(
            Rc::new(Gradient::from_ndarray(right)),
            Rc::new(Gradient::from_ndarray(grad)),
        );

        op.backward();
    }

    #[test]
    fn backward() -> Result<(), Box<dyn Error>> {
        let left = Array::zeros((3, 3));
        let right = Array::zeros((3, 3));
        let grad = Array::ones((3, 3));
        let shared_grad = Rc::new(Gradient::from_ndarray(grad.clone()));
        let op = AdditionBackward::new(
            AdditionBackwardLeft::<Ix2, Ix2>::new(
                Rc::new(Gradient::from_ndarray(left.clone())),
                shared_grad.clone(),
            ),
            AdditionBackwardRight::<Ix2, Ix2>::new(
                Rc::new(Gradient::from_ndarray(right.clone())),
                shared_grad,
            ),
        );

        op.backward();
        are_similar(op.left.operand_gradient.borrow(), &(&left + &grad))?;
        are_similar(op.left.gradient.borrow(), &grad)?;
        are_similar(op.right.operand_gradient.borrow(), &(&grad + &right))?;
        are_similar(op.right.gradient.borrow(), &grad)?;

        op.backward();
        are_similar(op.left.operand_gradient.borrow(), &(&left + &grad * 2.))?;
        are_similar(op.left.gradient.borrow(), &grad)?;
        are_similar(op.right.operand_gradient.borrow(), &(2. * &grad + &right))?;
        are_similar(op.right.gradient.borrow(), &grad)
    }
}
