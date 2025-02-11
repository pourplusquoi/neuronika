use ndarray::{Ix1, Ix2};
use neuronika::{data::DataLoader, nn::Linear, optim, MatMatMulT, Reduction, VarDiff};

#[cfg_attr(feature = "serialize", derive(serde::Serialize, serde::Deserialize))]
struct NeuralNetwork {
    lin1: Linear,
    lin2: Linear,
    lin3: Linear,
}

impl NeuralNetwork {
    fn parameters<T>(&self, optimizer: &optim::Optimizer<T>)
    where
        T: optim::OptimizerStatus,
        VarDiff<Ix2>: optim::IntoParam<T>,
        VarDiff<Ix1>: optim::IntoParam<T>,
    {
        optimizer.register(self.lin1.weight.clone());
        optimizer.register(self.lin1.bias.clone());

        optimizer.register(self.lin2.weight.clone());
        optimizer.register(self.lin2.bias.clone());

        optimizer.register(self.lin3.weight.clone());
        optimizer.register(self.lin3.bias.clone());
    }

    fn forward<I>(&self, input: I) -> VarDiff<Ix2>
    where
        I: MatMatMulT<VarDiff<Ix2>>,
        I::Output: Into<VarDiff<Ix2>>,
    {
        let out1 = self.lin1.forward(input).relu();
        let out2 = self.lin2.forward(out1).relu();
        self.lin3.forward(out2)
    }
}

/// Loads model from a string.
fn load_model() -> NeuralNetwork {
    serde_json::from_str::<NeuralNetwork>(
        r#"
        {
            "lin1":{
               "weight":{
                  "v":1,
                  "dim":[
                     5,
                     3
                  ],
                  "data":[
                     0.31398147,
                     -0.02374097,
                     -0.045672387,
                     -0.57606286,
                     0.5287176,
                     -0.038059983,
                     -0.19196294,
                     0.9338395,
                     -0.34874597,
                     -0.08579302,
                     -0.21880743,
                     0.26289353,
                     0.12593554,
                     -0.19557185,
                     -0.6770759
                  ]
               },
               "bias":{
                  "v":1,
                  "dim":[
                     5
                  ],
                  "data":[
                     0.036578782,
                     -0.3663301,
                     -0.23192844,
                     0.1254652,
                     0.5213851
                  ]
               }
            },
            "lin2":{
               "weight":{
                  "v":1,
                  "dim":[
                     5,
                     5
                  ],
                  "data":[
                     0.52091986,
                     0.3500197,
                     -0.06102618,
                     -0.43995684,
                     0.53706765,
                     -0.09257236,
                     -0.3584929,
                     -0.43666622,
                     0.43744308,
                     -0.40631944,
                     0.066774696,
                     0.16129021,
                     -0.25963476,
                     0.26902968,
                     0.1528883,
                     0.12935583,
                     -0.2496377,
                     0.14702061,
                     -0.012540738,
                     -0.34052926,
                     0.45684096,
                     -0.12884608,
                     0.21005273,
                     -0.7786633,
                     -0.08895902
                  ]
               },
               "bias":{
                  "v":1,
                  "dim":[
                     5
                  ],
                  "data":[
                     0.6071196,
                     -0.18910336,
                     -0.2278286,
                     0.044481196,
                     0.10841279
                  ]
               }
            },
            "lin3":{
               "weight":{
                  "v":1,
                  "dim":[
                     1,
                     5
                  ],
                  "data":[
                     0.21673596,
                     -0.021770507,
                     -0.00067504647,
                     0.5252394,
                     0.06640336
                  ]
               },
               "bias":{
                  "v":1,
                  "dim":[
                     1
                  ],
                  "data":[
                     0.7723236
                  ]
               }
            }
         }"#,
    )
    .unwrap()
}

fn main() {
    // Dataset.
    let csv_content = "\
        Paw_size,Tail_length,Weight,Animal\n\
        0.2,5.0,15.0,Dog\n\
        0.08,12.0,4.0,Cat\n\
        0.07,13.0,5.0,Cat\n\
        0.05,3.0,0.8,Mouse";

    // Creates data loader.
    let mut dataset = DataLoader::default().with_labels(&[3]).from_reader_fn(
        csv_content.as_bytes(),
        3,
        1,
        |(record, label): (Vec<f32>, String)| {
            let float_label = match label.as_str() {
                "Dog" => 1.,
                "Cat" => 2.,
                _ => 3.,
            };
            (record, vec![float_label])
        },
    );

    // Loads the model.
    let model = load_model();

    // Creates the optimizer.
    let optimizer = optim::StochasticGD::new(0.01, optim::L2::new(0.0), None, None, false);
    model.parameters(&optimizer);

    // Trains the model.
    for epoch in 0..5 {
        let batched_data = dataset.shuffle().batch(2).drop_last();
        let mut total_loss: f32 = 0.0;

        for (input_array, target_array) in batched_data {
            let input = neuronika::from_ndarray(input_array.to_owned());
            let target = neuronika::from_ndarray(target_array.to_owned());

            let result = model.forward(input);

            let loss = result.mse(target, Reduction::Mean);
            loss.forward();
            total_loss += loss.data()[()];
            loss.backward(1.0);
            optimizer.step();
        }

        println!("Loss for epoch {} : {} ", epoch, total_loss);
    }
}
