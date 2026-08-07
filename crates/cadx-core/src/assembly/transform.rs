//! Rigid placements shared by occurrences and mate anchor frames.

use serde::{Deserialize, Serialize};

use super::AssemblyError;

/// A right-handed rigid placement from component-local to parent coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AssemblyTransform {
    pub translation: [f64; 3],
    /// Row-major orthonormal rotation matrix.
    pub rotation: [[f64; 3]; 3],
}

impl AssemblyTransform {
    pub const IDENTITY: Self = Self {
        translation: [0.0; 3],
        rotation: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
    };

    #[must_use]
    pub fn compose(self, local: Self) -> Self {
        let rotation = std::array::from_fn(|row| {
            std::array::from_fn(|column| {
                (0..3)
                    .map(|axis| self.rotation[row][axis] * local.rotation[axis][column])
                    .sum()
            })
        });
        let translation = std::array::from_fn(|row| {
            self.translation[row]
                + (0..3)
                    .map(|axis| self.rotation[row][axis] * local.translation[axis])
                    .sum::<f64>()
        });
        Self {
            translation,
            rotation,
        }
    }

    /// Returns the inverse parent-to-local rigid transform.
    #[must_use]
    pub fn inverse(self) -> Self {
        let rotation =
            std::array::from_fn(|row| std::array::from_fn(|column| self.rotation[column][row]));
        let translation = std::array::from_fn(|row| {
            -(0..3)
                .map(|axis| rotation[row][axis] * self.translation[axis])
                .sum::<f64>()
        });
        Self {
            translation,
            rotation,
        }
    }

    /// Applies this placement to a point, including translation.
    #[must_use]
    pub fn transform_point(self, point: [f64; 3]) -> [f64; 3] {
        std::array::from_fn(|row| {
            self.translation[row]
                + (0..3)
                    .map(|axis| self.rotation[row][axis] * point[axis])
                    .sum::<f64>()
        })
    }

    /// Applies only this placement's rotation to a direction vector.
    #[must_use]
    pub fn transform_vector(self, vector: [f64; 3]) -> [f64; 3] {
        std::array::from_fn(|row| {
            (0..3)
                .map(|axis| self.rotation[row][axis] * vector[axis])
                .sum()
        })
    }

    #[must_use]
    pub fn from_euler_xyz_degrees(translation: [f64; 3], rotation: [f64; 3]) -> Self {
        let [x, y, z] = rotation.map(f64::to_radians);
        let (sin_x, cos_x) = x.sin_cos();
        let (sin_y, cos_y) = y.sin_cos();
        let (sin_z, cos_z) = z.sin_cos();
        Self {
            translation,
            rotation: [
                [
                    cos_z * cos_y,
                    cos_z * sin_y * sin_x - sin_z * cos_x,
                    cos_z * sin_y * cos_x + sin_z * sin_x,
                ],
                [
                    sin_z * cos_y,
                    sin_z * sin_y * sin_x + cos_z * cos_x,
                    sin_z * sin_y * cos_x - cos_z * sin_x,
                ],
                [-sin_y, cos_y * sin_x, cos_y * cos_x],
            ],
        }
    }

    /// Converts `Rz * Ry * Rx` to the Euler convention used by solid features.
    #[must_use]
    pub fn euler_xyz_degrees(self) -> [f64; 3] {
        let sine_y = (-self.rotation[2][0]).clamp(-1.0, 1.0);
        let y = sine_y.asin();
        let cosine_y = y.cos();
        let (x, z) = if cosine_y.abs() > 1.0e-10 {
            (
                self.rotation[2][1].atan2(self.rotation[2][2]),
                self.rotation[1][0].atan2(self.rotation[0][0]),
            )
        } else {
            ((-self.rotation[1][2]).atan2(self.rotation[1][1]), 0.0)
        };
        [x.to_degrees(), y.to_degrees(), z.to_degrees()]
    }

    #[must_use]
    pub fn approximately_equals(self, other: Self, tolerance: f64) -> bool {
        self.translation
            .into_iter()
            .zip(other.translation)
            .chain(
                self.rotation
                    .into_iter()
                    .flatten()
                    .zip(other.rotation.into_iter().flatten()),
            )
            .all(|(left, right)| (left - right).abs() <= tolerance)
    }

    pub(crate) fn validate(self) -> Result<(), AssemblyError> {
        const TOLERANCE: f64 = 1.0e-9;

        if self
            .translation
            .into_iter()
            .chain(self.rotation.into_iter().flatten())
            .any(|value| !value.is_finite())
        {
            return Err(AssemblyError::NonFiniteTransform);
        }
        for row in 0..3 {
            for other in 0..3 {
                let dot = (0..3)
                    .map(|axis| self.rotation[row][axis] * self.rotation[other][axis])
                    .sum::<f64>();
                let expected = if row == other { 1.0 } else { 0.0 };
                if (dot - expected).abs() > TOLERANCE {
                    return Err(AssemblyError::NonRigidTransform);
                }
            }
        }
        let determinant = self.rotation[0][0]
            * (self.rotation[1][1] * self.rotation[2][2]
                - self.rotation[1][2] * self.rotation[2][1])
            - self.rotation[0][1]
                * (self.rotation[1][0] * self.rotation[2][2]
                    - self.rotation[1][2] * self.rotation[2][0])
            + self.rotation[0][2]
                * (self.rotation[1][0] * self.rotation[2][1]
                    - self.rotation[1][1] * self.rotation[2][0]);
        if (determinant - 1.0).abs() > TOLERANCE {
            return Err(AssemblyError::NonRigidTransform);
        }
        Ok(())
    }
}

impl Default for AssemblyTransform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rigid_transform_composes_and_converts_to_feature_euler_angles() {
        let rotation_z_90 = AssemblyTransform {
            translation: [10.0, 0.0, 0.0],
            rotation: [[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
        };
        let local = AssemblyTransform {
            translation: [2.0, 0.0, 0.0],
            ..AssemblyTransform::IDENTITY
        };
        let world = rotation_z_90.compose(local);
        assert!(
            world
                .translation
                .into_iter()
                .zip([10.0, 2.0, 0.0])
                .all(|(actual, expected)| (actual - expected).abs() < 1.0e-9)
        );
        assert!(
            world
                .euler_xyz_degrees()
                .into_iter()
                .zip([0.0, 0.0, 90.0])
                .all(|(actual, expected)| (actual - expected).abs() < 1.0e-9)
        );
        world.validate().unwrap();

        let point = [3.0, -2.0, 5.0];
        let transformed = world.transform_point(point);
        let restored = world.inverse().transform_point(transformed);
        assert!(
            restored
                .into_iter()
                .zip(point)
                .all(|(actual, expected)| (actual - expected).abs() < 1.0e-9)
        );
        let direction = [0.25, -0.5, 0.75];
        let restored_direction = world
            .inverse()
            .transform_vector(world.transform_vector(direction));
        assert!(
            restored_direction
                .into_iter()
                .zip(direction)
                .all(|(actual, expected)| (actual - expected).abs() < 1.0e-9)
        );
    }

    #[test]
    fn reflections_and_scaled_placements_are_not_rigid() {
        for rotation in [
            [[-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            [[2.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        ] {
            assert!(matches!(
                AssemblyTransform {
                    translation: [0.0; 3],
                    rotation,
                }
                .validate(),
                Err(AssemblyError::NonRigidTransform)
            ));
        }
    }

    #[test]
    fn feature_euler_round_trip_preserves_rigid_matrix() {
        let source = AssemblyTransform::from_euler_xyz_degrees([1.0, 2.0, 3.0], [17.0, 31.0, 73.0]);
        let rebuilt = AssemblyTransform::from_euler_xyz_degrees(
            source.translation,
            source.euler_xyz_degrees(),
        );
        assert!(source.approximately_equals(rebuilt, 1.0e-12));
    }
}
