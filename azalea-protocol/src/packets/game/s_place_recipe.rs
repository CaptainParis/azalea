use azalea_buf::AzBuf;
use azalea_protocol_macros::ServerboundGamePacket;

#[derive(AzBuf, Clone, Debug, PartialEq, ServerboundGamePacket)]
pub struct ServerboundPlaceRecipe {
    #[var]
    pub container_id: i32,
    /// The `id` of a [`RecipeDisplayEntry`] from `ClientboundRecipeBookAdd`.
    ///
    /// [`RecipeDisplayEntry`]: crate::packets::game::c_recipe_book_add::RecipeDisplayEntry
    #[var]
    pub recipe: u32,
    pub shift_down: bool,
}
